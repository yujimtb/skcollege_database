//! M06: Propagation Scheduler — poll-based incremental propagation.

use crate::domain::{ProjectionHealth, ProjectionRef};
use crate::lake::LakeStore;
use crate::projection::catalog::ProjectionCatalog;
use crate::projection::runner::BuildStatus;

use super::watermark::WatermarkStore;

/// Result of a single propagation cycle for one projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropagationResult {
    /// No new data — skipped.
    NoOp,
    /// Incremental apply executed.
    Applied { new_position: usize, new_records: usize },
    /// Build failed — watermark unchanged.
    Failed { reason: String },
}

/// Poll-based propagation scheduler (M06 §4.1 MVP).
pub struct PropagationScheduler;

impl PropagationScheduler {
    /// Run a single poll cycle for one projection.
    ///
    /// Returns the incremental observations and the new position.
    /// The caller is responsible for running the actual projector.
    pub fn check_and_prepare(
        projection_id: &ProjectionRef,
        lake: &LakeStore,
        watermarks: &mut WatermarkStore,
    ) -> (usize, usize) {
        let wm = watermarks.get_or_init(projection_id);
        let current_pos = wm.last_processed_position;
        let lake_pos = lake.watermark().map(|w| w.position).unwrap_or(0);

        watermarks.update_pending(projection_id, lake_pos);
        (current_pos, lake_pos)
    }

    /// Commit a successful incremental apply.
    pub fn commit_success(
        projection_id: &ProjectionRef,
        new_position: usize,
        watermarks: &mut WatermarkStore,
        catalog: &mut ProjectionCatalog,
    ) {
        watermarks.update(projection_id, new_position, BuildStatus::Success);
        catalog.set_health(projection_id, ProjectionHealth::Healthy);
    }

    /// Record a failed build — watermark unchanged (M06 invariant 4).
    pub fn commit_failure(
        projection_id: &ProjectionRef,
        watermarks: &mut WatermarkStore,
        catalog: &mut ProjectionCatalog,
    ) {
        watermarks.record_failure(projection_id);
        catalog.set_health(projection_id, ProjectionHealth::Broken);
    }

    /// Run propagation in topological order for all projections.
    /// Returns ids of projections that had new data applied.
    pub fn propagate_all(
        lake: &LakeStore,
        watermarks: &mut WatermarkStore,
        catalog: &mut ProjectionCatalog,
    ) -> Result<Vec<(ProjectionRef, PropagationResult)>, crate::projection::catalog::CatalogError> {
        let order = match catalog.topological_order() {
            Ok(o) => o,
            Err(err) => {
                for proj_id in catalog.list_ids() {
                    catalog.set_health(&proj_id, ProjectionHealth::Broken);
                }
                return Err(err);
            }
        };

        let mut results = Vec::new();

        for proj_id in &order {
            let (current, lake_pos) = Self::check_and_prepare(proj_id, lake, watermarks);
            if current >= lake_pos {
                results.push((proj_id.clone(), PropagationResult::NoOp));
                continue;
            }

            let new_records = lake_pos - current;

            // In a real system the projector would be called here.
            // For the MVP framework, we just advance the watermark.
            Self::commit_success(proj_id, lake_pos, watermarks, catalog);
            results.push((
                proj_id.clone(),
                PropagationResult::Applied {
                    new_position: lake_pos,
                    new_records,
                },
            ));
        }

        Ok(results)
    }

    /// Mark downstream projections as degraded when an upstream fails.
    pub fn propagate_upstream_failure(
        failed_id: &ProjectionRef,
        catalog: &mut ProjectionCatalog,
    ) {
        let dependents = catalog.dependents(failed_id);
        for dep_id in &dependents {
            catalog.set_health(dep_id, ProjectionHealth::Degraded);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::*;
    use crate::lake::LakeStore;
    use crate::projection::catalog::ProjectionCatalog;
    use crate::projection::spec::*;

    fn sample_obs(key: &str) -> Observation {
        Observation {
            id: Observation::new_id(),
            schema: SchemaRef::new("schema:test"),
            schema_version: SemVer::new("1.0.0"),
            observer: ObserverRef::new("obs:test"),
            source_system: None,
            actor: None,
            authority_model: AuthorityModel::LakeAuthoritative,
            capture_model: CaptureModel::Event,
            subject: EntityRef::new("msg:1"),
            target: None,
            payload: serde_json::json!({}),
            attachments: vec![],
            published: chrono::Utc::now(),
            recorded_at: chrono::Utc::now(),
            consent: None,
            idempotency_key: Some(IdempotencyKey::new(key)),
            meta: serde_json::json!({}),
        }
    }

    fn lake_spec(id: &str) -> ProjectionSpec {
        ProjectionSpec {
            id: ProjectionRef::new(id),
            name: id.into(),
            version: SemVer::new("1.0.0"),
            kind: ProjectionKind::PureProjection,
            sources: vec![SourceDecl {
                source: SourceRef::Lake,
                filter_schemas: vec![],
                filter_derivations: vec![],
            }],
            read_modes: vec![ReadModePolicy {
                mode: ReadMode::OperationalLatest,
                source_policy: "lake-latest".into(),
            }],
            build: BuildSpec {
                build_type: "rust".into(),
                entrypoint: None,
                projector: "p".into(),
            },
            outputs: vec![OutputSpec { format: "sql".into(), tables: vec!["t".into()] }],
            reconciliation: None,
            deterministic_in: vec![],
            gap_action: None,
            tags: vec![],
            description: None,
            created_by: "test".into(),
        }
    }

    #[test]
    fn no_new_data_is_noop() {
        let lake = LakeStore::new();
        let mut watermarks = WatermarkStore::new();
        let mut catalog = ProjectionCatalog::new();
        catalog.register(lake_spec("proj:a")).unwrap();

        let results = PropagationScheduler::propagate_all(&lake, &mut watermarks, &mut catalog)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1, PropagationResult::NoOp);
    }

    #[test]
    fn new_observations_trigger_apply() {
        let mut lake = LakeStore::new();
        lake.append(sample_obs("k1")).unwrap();
        lake.append(sample_obs("k2")).unwrap();

        let mut watermarks = WatermarkStore::new();
        let mut catalog = ProjectionCatalog::new();
        catalog.register(lake_spec("proj:a")).unwrap();

        let results = PropagationScheduler::propagate_all(&lake, &mut watermarks, &mut catalog)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert!(matches!(
            results[0].1,
            PropagationResult::Applied { new_position: 2, new_records: 2 }
        ));

        // Second run should be no-op.
        let results2 = PropagationScheduler::propagate_all(&lake, &mut watermarks, &mut catalog)
            .unwrap();
        assert_eq!(results2[0].1, PropagationResult::NoOp);
    }

    #[test]
    fn incremental_after_initial() {
        let mut lake = LakeStore::new();
        lake.append(sample_obs("k1")).unwrap();

        let mut watermarks = WatermarkStore::new();
        let mut catalog = ProjectionCatalog::new();
        catalog.register(lake_spec("proj:a")).unwrap();

        PropagationScheduler::propagate_all(&lake, &mut watermarks, &mut catalog).unwrap();

        // Add more data.
        lake.append(sample_obs("k2")).unwrap();
        lake.append(sample_obs("k3")).unwrap();

        let results = PropagationScheduler::propagate_all(&lake, &mut watermarks, &mut catalog)
            .unwrap();
        assert!(matches!(
            results[0].1,
            PropagationResult::Applied { new_position: 3, new_records: 2 }
        ));
    }

    #[test]
    fn cycle_error_is_returned_and_health_is_broken() {
        let lake = LakeStore::new();
        let mut watermarks = WatermarkStore::new();
        let mut catalog = ProjectionCatalog::new();
        catalog.register(lake_spec("proj:a")).unwrap();

        let mut dep = lake_spec("proj:b");
        dep.sources.insert(0, SourceDecl {
            source: SourceRef::Projection {
                id: ProjectionRef::new("proj:a"),
                version: ">=1.0.0".into(),
            },
            filter_schemas: vec![],
            filter_derivations: vec![],
        });
        dep.reconciliation = Some(ReconciliationPolicy::LakeFirst);
        catalog.register(dep).unwrap();

        let entry = catalog.get_mut(&ProjectionRef::new("proj:a")).unwrap();
        entry.spec.sources.insert(0, SourceDecl {
            source: SourceRef::Projection {
                id: ProjectionRef::new("proj:b"),
                version: ">=1.0.0".into(),
            },
            filter_schemas: vec![],
            filter_derivations: vec![],
        });
        entry.spec.reconciliation = Some(ReconciliationPolicy::LakeFirst);

        let err = PropagationScheduler::propagate_all(&lake, &mut watermarks, &mut catalog)
            .unwrap_err();
        assert_eq!(err, crate::projection::catalog::CatalogError::CyclicDependency);
        assert_eq!(
            catalog.get(&ProjectionRef::new("proj:a")).unwrap().health,
            ProjectionHealth::Broken
        );
        assert_eq!(
            catalog.get(&ProjectionRef::new("proj:b")).unwrap().health,
            ProjectionHealth::Broken
        );
    }

    #[test]
    fn upstream_failure_degrades_downstream() {
        let mut catalog = ProjectionCatalog::new();
        catalog.register(lake_spec("proj:a")).unwrap();

        let mut dep = lake_spec("proj:b");
        dep.sources.insert(0, SourceDecl {
            source: SourceRef::Projection {
                id: ProjectionRef::new("proj:a"),
                version: ">=1.0.0".into(),
            },
            filter_schemas: vec![],
            filter_derivations: vec![],
        });
        dep.reconciliation = Some(ReconciliationPolicy::LakeFirst);
        catalog.register(dep).unwrap();

        PropagationScheduler::propagate_upstream_failure(
            &ProjectionRef::new("proj:a"),
            &mut catalog,
        );

        let entry = catalog.get(&ProjectionRef::new("proj:b")).unwrap();
        assert_eq!(entry.health, ProjectionHealth::Degraded);
    }
}
