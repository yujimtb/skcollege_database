use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};

use crate::adapter::config::{AdapterConfig, BackoffStrategy, RateLimitConfig, RetryConfig, SchemaBinding};
use crate::adapter::gslides::client::GoogleSlidesClient;
use crate::adapter::gslides::mapper::GoogleSlidesAdapter;
use crate::adapter::slack::client::SlackClient;
use crate::adapter::slack::mapper::SlackAdapter;
use crate::adapter::traits::{ObservationDraft, SourceAdapter};
use crate::adapter::writeback::notion::client::{NotionClient, NotionConfig};
use crate::adapter::writeback::traits::SaaSWriteAdapter;
use crate::api::envelope::{ProjectionMetadata, ResponseEnvelope};
use crate::api::health::HealthResponse;
use crate::api::pagination::{paginate, PaginatedResponse, PaginationParams};
use crate::api::read_mode::{ReadModeError, ReadModeResolver};
use crate::domain::{
    ActorRef, AuthorityModel, BlobRef, CaptureModel, EntityRef, IngestResult, Observation,
    ObserverRef, ProjectionRef, ProjectionStatus, ReadMode, SchemaRef, SemVer,
    SourceSystemRef,
};
use crate::governance::engine::PolicyEngine;
use crate::governance::filter::FilteringGate;
use crate::governance::types::{
    AccessScope, ConsentStatus, Environment, MaskStrategy, Operation, PolicyOutcome,
    PolicyRequest, RestrictedFieldSpec, Role,
};
use crate::identity::projector::IdentityProjector;
use crate::identity::types::IdentityResolutionOutput;
use crate::lake::{BlobStore, IngestRequest, IngestionGate, LakeStore};
use crate::person_page::projector::PersonPageProjector;
use crate::person_page::types::{PersonDetailResponse, PersonListItem, PersonPageOutput, TimelineEvent};
use crate::projection::catalog::ProjectionCatalog;
use crate::projection::runner::Projector;
use crate::self_host::config::SelfHostConfig;
use crate::self_host::google::HttpGoogleSlidesClient;
use crate::self_host::persistence::{PersistenceError, SqlitePersistence};
use crate::self_host::registry::{seed_projection_catalog, seed_registry};
use crate::self_host::slack::HttpSlackClient;
use crate::slide_analysis::GeminiSlideAnalyzer;
use crate::supplemental::SupplementalStore;

#[derive(Debug, thiserror::Error)]
pub enum SelfHostError {
    #[error(transparent)]
    Config(#[from] crate::self_host::config::ConfigError),
    #[error(transparent)]
    Persistence(#[from] PersistenceError),
    #[error(transparent)]
    Adapter(#[from] crate::adapter::error::AdapterError),
    #[error("read mode error: {0}")]
    ReadMode(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("policy denied: {0}")]
    Policy(String),
    #[error("internal state lock poisoned")]
    LockPoisoned,
    #[error("ingestion rejected: {0}")]
    Ingestion(String),
    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SyncReport {
    pub slack_ingested: usize,
    pub google_ingested: usize,
    pub slide_analyses: usize,
    pub notion_synced: usize,
    pub duplicates: usize,
    pub last_sync_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ProjectionSnapshot {
    pub identity: IdentityResolutionOutput,
    pub person_page: PersonPageOutput,
    pub built_at: DateTime<Utc>,
}

impl Default for ProjectionSnapshot {
    fn default() -> Self {
        Self {
            identity: IdentityResolutionOutput::default(),
            person_page: PersonPageOutput::default(),
            built_at: Utc::now(),
        }
    }
}

#[derive(Debug)]
pub struct AppCore {
    pub registry: crate::registry::RegistryStore,
    pub catalog: ProjectionCatalog,
    pub lake: LakeStore,
    pub blobs: BlobStore,
    pub supplemental: SupplementalStore,
    pub snapshot: ProjectionSnapshot,
    pub last_sync_at: Option<DateTime<Utc>>,
    pub last_sync_error: Option<String>,
}

impl AppCore {
    fn new(
        observations: Vec<Observation>,
        persisted_blobs: Vec<Vec<u8>>,
        persisted_supplementals: Vec<crate::domain::SupplementalRecord>,
    ) -> Result<Self, SelfHostError> {
        let mut lake = LakeStore::new();
        for observation in observations {
            lake.append(observation).map_err(|existing_id| {
                SelfHostError::Ingestion(format!(
                    "duplicate persisted observation detected during bootstrap: {existing_id}"
                ))
            })?;
        }

        let mut blobs = BlobStore::new();
        for blob in persisted_blobs {
            blobs.put(&blob);
        }

        let mut supplemental = SupplementalStore::new();
        for record in persisted_supplementals {
            supplemental.upsert(record, &lake).map_err(|err| {
                SelfHostError::Ingestion(format!(
                    "invalid persisted supplemental detected during bootstrap: {err}"
                ))
            })?;
        }

        let mut core = Self {
            registry: seed_registry(),
            catalog: seed_projection_catalog(),
            lake,
            blobs,
            supplemental,
            snapshot: ProjectionSnapshot::default(),
            last_sync_at: None,
            last_sync_error: None,
        };
        core.rebuild_snapshot();
        Ok(core)
    }

    fn rebuild_snapshot(&mut self) {
        let identity = IdentityProjector::new("1.0.0")
            .project(self.lake.list())
            .into_iter()
            .next()
            .unwrap_or_default();
        let supplemental_records = self.supplemental.by_kind("slide-analysis");
        let person_page = PersonPageProjector::project(&identity, self.lake.list(), &supplemental_records);
        self.snapshot = ProjectionSnapshot {
            identity,
            person_page,
            built_at: Utc::now(),
        };
        self.catalog
            .set_status(&ProjectionRef::new("proj:identity-resolution"), ProjectionStatus::Active);
        self.catalog
            .set_status(&ProjectionRef::new("proj:person-page"), ProjectionStatus::Active);
    }

    fn ingest(&mut self, draft: ObservationDraft) -> IngestResult {
        let request = IngestRequest {
            schema: draft.schema,
            schema_version: draft.schema_version,
            observer: draft.observer,
            source_system: draft.source_system,
            authority_model: draft.authority_model,
            capture_model: draft.capture_model,
            subject: draft.subject,
            target: draft.target,
            payload: draft.payload,
            attachments: draft.attachments,
            published: draft.published,
            idempotency_key: Some(draft.idempotency_key),
            meta: draft.meta,
        };

        let mut gate = IngestionGate {
            registry: &self.registry,
            lake: &mut self.lake,
            blobs: &self.blobs,
        };
        gate.ingest(request)
    }

    /// Add a supplemental record using this core's lake for validation.
    fn add_supplemental(
        &mut self,
        record: crate::domain::SupplementalRecord,
    ) -> Result<crate::domain::SupplementalId, crate::domain::DomainError> {
        self.supplemental.upsert(record, &self.lake)
    }
}

#[derive(Clone)]
pub struct AppService {
    core: Arc<Mutex<AppCore>>,
    persistence: Arc<Mutex<SqlitePersistence>>,
    config: Arc<SelfHostConfig>,
    slack_client: HttpSlackClient,
    slack_replies_client: HttpSlackClient,
    google_client: HttpGoogleSlidesClient,
    slide_analyzer: Option<GeminiSlideAnalyzer>,
    notion_client: Option<NotionClient>,
}

impl AppService {
    pub fn bootstrap(config: SelfHostConfig) -> Result<Self, SelfHostError> {
        let persistence = SqlitePersistence::open(&config.database_path, &config.blob_dir)?;
        let observations = persistence.load_observations()?;
        let blobs = persistence.load_blobs()?;
        let supplementals = persistence.load_supplementals()?;
        let slack_client = HttpSlackClient::new(config.slack.bot_token.clone())?;
        let slack_replies_client = HttpSlackClient::new(
            config
                .slack
                .thread_token
                .clone()
                .unwrap_or_else(|| config.slack.bot_token.clone()),
        )?;
        let google_client = HttpGoogleSlidesClient::new(&config.google)?;
        let slide_analyzer = config
            .slide_ai
            .as_ref()
            .map(|slide_ai| GeminiSlideAnalyzer::new(&slide_ai.api_key, &slide_ai.model))
            .transpose()?;

        let notion_client = config
            .notion
            .as_ref()
            .map(|nc| {
                NotionClient::new(NotionConfig::new(&nc.token, &nc.database_id))
            })
            .transpose()?;

        Ok(Self {
            core: Arc::new(Mutex::new(AppCore::new(
                observations,
                blobs,
                supplementals,
            )?)),
            persistence: Arc::new(Mutex::new(persistence)),
            config: Arc::new(config),
            slack_client,
            slack_replies_client,
            google_client,
            slide_analyzer,
            notion_client,
        })
    }

    pub fn spawn_polling_task(&self) {
        let service = self.clone();
        let interval = self.config.poll_interval;
        tokio::spawn(async move {
            loop {
                let cloned = service.clone();
                let result = tokio::task::spawn_blocking(move || cloned.sync_all()).await;
                if let Err(err) = result {
                    eprintln!("poll task join error: {err}");
                } else if let Ok(Err(err)) = result {
                    eprintln!("poll sync error: {err}");
                }
                tokio::time::sleep(interval).await;
            }
        });
    }

    pub fn sync_all(&self) -> Result<SyncReport, SelfHostError> {
        let mut slack_ingested = 0usize;
        let mut google_ingested = 0usize;
        let mut duplicates = 0usize;

        let slack_adapter = SlackAdapter::new(self.slack_client.clone(), self.slack_adapter_config());
        for channel_id in &self.config.slack.channel_ids {
            let cursor_key = format!("slack:{channel_id}:oldest_ts");
            let oldest = non_empty_state(self.persistence_lock()?.get_state(&cursor_key)?);
            let mut page_cursor: Option<String> = None;
            let mut latest_ts = oldest.clone();

            loop {
                let page = self
                    .slack_client
                    .conversations_history(channel_id, oldest.as_deref(), page_cursor.as_deref(), 200)?;
                for message in page.messages {
                    let thread_root = thread_root_ts(&message).map(str::to_owned);
                    match self.ingest_slack_message(
                        &slack_adapter,
                        &self.slack_client,
                        channel_id,
                        message,
                        &mut latest_ts,
                    )? {
                        IngestResult::Ingested { .. } => slack_ingested += 1,
                        IngestResult::Duplicate { .. } => duplicates += 1,
                        _ => {}
                    }

                    if let Some(thread_ts) = thread_root {
                        let replies = self
                            .slack_replies_client
                            .conversations_replies(channel_id, &thread_ts)?;
                        for reply in replies
                            .into_iter()
                            .filter(|reply| reply.ts != thread_ts)
                        {
                            match self.ingest_slack_message(
                                &slack_adapter,
                                &self.slack_replies_client,
                                channel_id,
                                reply,
                                &mut latest_ts,
                            )? {
                                IngestResult::Ingested { .. } => slack_ingested += 1,
                                IngestResult::Duplicate { .. } => duplicates += 1,
                                _ => {}
                            }
                        }
                    }
                }
                if page.has_more {
                    page_cursor = page.next_cursor;
                } else {
                    break;
                }
            }

            let channel_snapshot = self.slack_client.conversations_info(channel_id)?;
            match self.ingest_draft(slack_adapter.map_channel_snapshot(&channel_snapshot))? {
                IngestResult::Ingested { .. } => slack_ingested += 1,
                IngestResult::Duplicate { .. } => duplicates += 1,
                _ => {}
            }

            if let Some(latest_ts) = latest_ts.as_deref() {
                self.persistence_lock()?.set_state(&cursor_key, latest_ts)?;
            }
        }

        match self.ingest_draft(slack_adapter.heartbeat())? {
            IngestResult::Ingested { .. } => slack_ingested += 1,
            IngestResult::Duplicate { .. } => duplicates += 1,
            _ => {}
        }

        let google_adapter = GoogleSlidesAdapter::new(self.google_client.clone(), self.google_adapter_config());
        for presentation_id in &self.config.google.presentation_ids {
            let cursor_key = format!("gslides:{presentation_id}:revision");
            let last_revision = self.persistence_lock()?.get_state(&cursor_key)?;

            let mut page_token: Option<String> = None;
            let mut revisions = Vec::new();
            loop {
                let page = self
                    .google_client
                    .list_revisions(presentation_id, page_token.as_deref())?;
                revisions.extend(page.revisions);
                if let Some(token) = page.next_page_token {
                    page_token = Some(token);
                } else {
                    break;
                }
            }
            revisions.sort_by_key(|revision| revision.modified_time);

            let should_reset = last_revision.as_ref().is_some_and(|needle| {
                !revisions.iter().any(|revision| revision.revision_id == *needle)
            });
            let new_revisions = revisions_after_cursor(revisions, last_revision.as_deref(), should_reset);

            let Some(captured_revision) = latest_revision_to_capture(&new_revisions).cloned() else {
                continue;
            };

            let meta = self.google_client.get_presentation_meta(presentation_id)?;
            let presentation = self.google_client.get_presentation(presentation_id)?;
            let native_blob = self.store_blob(&serde_json::to_vec(&presentation)?)?;
            let rendered_blobs = presentation
                .slides
                .first()
                .map(|slide| self.google_client.render_slide(presentation_id, &slide.object_id, "png"))
                .transpose()?
                .map(|rendered| self.store_blob(&rendered.data))
                .transpose()?
                .into_iter()
                .collect::<Vec<_>>();

            match self.ingest_draft(google_adapter.map_revision(
                &captured_revision,
                &meta,
                Some(native_blob),
                rendered_blobs,
            ))? {
                IngestResult::Ingested { .. } => google_ingested += 1,
                IngestResult::Duplicate { .. } => duplicates += 1,
                _ => {}
            }

            self.persistence_lock()?
                .set_state(&cursor_key, &captured_revision.revision_id)?;
        }

        match self.ingest_draft(google_adapter.heartbeat())? {
            IngestResult::Ingested { .. } => google_ingested += 1,
            IngestResult::Duplicate { .. } => duplicates += 1,
            _ => {}
        }

        let last_sync_at = Utc::now();
        let mut core = self.core_lock()?;
        core.last_sync_at = Some(last_sync_at);
        core.last_sync_error = None;
        let should_rebuild_snapshot = slack_ingested > 0 || google_ingested > 0;

        let schema = crate::domain::SchemaRef::new("schema:workspace-object-snapshot");
        let slide_observations: Vec<crate::domain::Observation> = core
            .lake
            .by_schema(&schema)
            .into_iter()
            .cloned()
            .collect();
        let slide_obs_by_presentation = slide_observations
            .iter()
            .fold(HashMap::<String, crate::domain::Observation>::new(), |mut acc, obs| {
                let Some(presentation_id) = obs
                    .payload
                    .pointer("/artifact/sourceObjectId")
                    .and_then(|value| value.as_str())
                else {
                    return acc;
                };

                match acc.get(presentation_id) {
                    Some(existing) if existing.published >= obs.published => {}
                    _ => {
                        acc.insert(presentation_id.to_string(), obs.clone());
                    }
                }
                acc
            });
        let slide_analysis_records: Vec<crate::domain::SupplementalRecord> = core
            .supplemental
            .by_kind("slide-analysis")
            .into_iter()
            .cloned()
            .collect();
        let analysis_model = self
            .slide_analyzer
            .as_ref()
            .map(|analyzer| format!("{}+continuation-v1", analyzer.model_name()))
            .unwrap_or_else(|| "heuristic-fallback+continuation-v1".to_string());
        let mut needs_analysis = false;
        for presentation_id in &self.config.google.presentation_ids {
            let Some(_observation) = slide_obs_by_presentation.get(presentation_id) else {
                continue;
            };
            let presentation = self.google_client.get_presentation(presentation_id)?;

            if presentation
                .slides
                .iter()
                .take(self.config.slide_analysis_limit)
                .any(|slide| match find_slide_analysis_record(&slide_analysis_records, presentation_id, &slide.object_id) {
                    Some(record) if self.slide_analyzer.is_some() => analysis_record_needs_refresh(record, &analysis_model),
                    Some(_) => false,
                    None => true,
                })
            {
                needs_analysis = true;
                break;
            }
        }

        // --- Slide Analysis + Notion write-back ---
        let mut slide_analyses = 0usize;
        let mut notion_synced = 0usize;

        if google_ingested > 0 || slack_ingested > 0 || needs_analysis {
            let mut analysis_results = Vec::new();

            for presentation_id in &self.config.google.presentation_ids {
                let Some(observation) = slide_obs_by_presentation.get(presentation_id) else {
                    continue;
                };

                let presentation = self.google_client.get_presentation(presentation_id)?;
                let canonical_uri = observation
                    .payload
                    .pointer("/artifact/canonicalUri")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string();

                let slides: Vec<_> = presentation
                    .slides
                    .iter()
                    .take(self.config.slide_analysis_limit)
                    .cloned()
                    .collect();
                let mut slide_index = 0usize;

                while slide_index < slides.len() {
                    let slide = &slides[slide_index];
                    if let Some(existing) = find_slide_analysis_record(&slide_analysis_records, presentation_id, &slide.object_id) {
                        if !self.slide_analyzer.is_some() || !analysis_record_needs_refresh(existing, &analysis_model) {
                            slide_index += 1;
                            continue;
                        }
                    }

                    let rendered = self.google_client.render_slide(presentation_id, &slide.object_id, "png")?;
                    let thumbnail_blob_ref = core.blobs.put(&rendered.data);
                    self.persistence_lock()?.persist_blob(&rendered.data)?;
                    let Some(mut profile) = self
                        .extract_student_profile_from_png(
                            &rendered.data,
                            observation,
                            &canonical_uri,
                        )
                        .or_else(|| heuristic_profile(observation)) else {
                        slide_index += 1;
                        continue;
                    };

                    profile.source_slide_object_id = Some(slide.object_id.clone());
                    profile.source_document_id = Some(format!(
                        "document:gslides:{presentation_id}#slide:{}",
                        slide.object_id
                    ));
                    profile.source_canonical_uri = Some(canonical_uri.clone());
                    profile.thumbnail_blob_ref = Some(thumbnail_blob_ref.as_str().to_string());
                    profile.thumbnail_url = rendered.content_url.clone();
                    profile.companion_to_slide_object_id = None;

                    let mut consumed_companion = false;
                    let mut companion_result = None;

                    if let Some(next_slide) = slides.get(slide_index + 1) {
                        let companion_rendered = self.google_client.render_slide(presentation_id, &next_slide.object_id, "png")?;
                        let Some(mut companion_profile) = self
                            .extract_student_profile_from_png(
                                &companion_rendered.data,
                                observation,
                                &canonical_uri,
                            )
                            .or_else(|| heuristic_profile(observation)) else {
                                slide_index += 1;
                                continue;
                            };

                        companion_profile.source_slide_object_id = Some(next_slide.object_id.clone());
                        companion_profile.source_document_id = Some(format!(
                            "document:gslides:{presentation_id}#slide:{}",
                            next_slide.object_id
                        ));
                        companion_profile.source_canonical_uri = Some(canonical_uri.clone());
                        companion_profile.thumbnail_url = companion_rendered.content_url.clone();
                        companion_profile.companion_to_slide_object_id = Some(slide.object_id.clone());

                        if should_merge_companion_slide(&profile, &companion_profile, observation) {
                            let companion_blob_ref = core.blobs.put(&companion_rendered.data);
                            self.persistence_lock()?.persist_blob(&companion_rendered.data)?;
                            companion_profile.thumbnail_blob_ref = Some(companion_blob_ref.as_str().to_string());
                            merge_companion_profile(&mut profile, &companion_profile);
                            consumed_companion = true;
                        }
                    }

                    ensure_profile_identifier(&mut profile, &slide.object_id);

                    let email = profile
                        .email
                        .as_deref()
                        .or(profile.generated_email.as_deref())
                        .map(ToOwned::to_owned)
                        .or_else(|| profile.source_document_id.clone())
                        .unwrap_or_else(|| "unknown".to_string());
                    let person_entity = EntityRef::new(format!("person:{email}"));
                    analysis_results.push(crate::slide_analysis::types::SlideAnalysisResult {
                        source_observation_id: observation.id.clone(),
                        presentation_id: presentation_id.clone(),
                        profile: profile.clone(),
                        person_entity: person_entity.clone(),
                        supplemental_id: Some(crate::domain::SupplementalId::new(format!(
                            "sup:slide-analysis:{presentation_id}:{}",
                            slide.object_id
                        ))),
                        analyzed_at: Utc::now(),
                        model_version: Some(analysis_model.clone()),
                        slide_object_id: Some(slide.object_id.clone()),
                        thumbnail_blob_ref: Some(thumbnail_blob_ref),
                    });

                    if consumed_companion {
                        if let Some(next_slide) = slides.get(slide_index + 1) {
                            let mut companion_profile = profile.clone();
                            companion_profile.source_slide_object_id = Some(next_slide.object_id.clone());
                            companion_profile.source_document_id = Some(format!(
                                "document:gslides:{presentation_id}#slide:{}",
                                next_slide.object_id
                            ));
                            companion_profile.companion_to_slide_object_id = Some(slide.object_id.clone());
                            companion_profile.thumbnail_blob_ref = None;
                            companion_profile.profile_pic = None;
                            companion_result = Some(crate::slide_analysis::types::SlideAnalysisResult {
                                source_observation_id: observation.id.clone(),
                                presentation_id: presentation_id.clone(),
                                profile: companion_profile,
                                person_entity,
                                supplemental_id: Some(crate::domain::SupplementalId::new(format!(
                                    "sup:slide-analysis:{presentation_id}:{}",
                                    next_slide.object_id
                                ))),
                                analyzed_at: Utc::now(),
                                model_version: Some(analysis_model.clone()),
                                slide_object_id: Some(next_slide.object_id.clone()),
                                thumbnail_blob_ref: None,
                            });
                        }
                    }

                    if let Some(companion_result) = companion_result {
                        analysis_results.push(companion_result);
                    }

                    slide_index += if consumed_companion { 2 } else { 1 };
                }
            }

            slide_analyses = analysis_results.len();

            for result in &analysis_results {
                let record = crate::slide_analysis::SlideAnalysisProjector::build_supplemental(result);
                core.add_supplemental(record.clone())
                    .map_err(|err| SelfHostError::Ingestion(err.to_string()))?;
                self.persistence_lock()?.persist_supplemental(&record)?;
            }

            for result in &analysis_results {
                let draft = crate::slide_analysis::SlideAnalysisProjector::create_analysis_observation(result);
                match core.ingest(draft) {
                    IngestResult::Ingested { id, .. } => {
                        let observation = core.lake.get(&id).cloned().ok_or_else(|| {
                            SelfHostError::Ingestion(format!(
                                "observation {id} missing after append"
                            ))
                        })?;
                        self.persistence_lock()?.persist_observation(&observation)?;
                    }
                    IngestResult::Rejected { message, .. } => {
                        return Err(SelfHostError::Ingestion(message));
                    }
                    IngestResult::Quarantined { ticket } => {
                        return Err(SelfHostError::Ingestion(ticket.reason));
                    }
                    IngestResult::Duplicate { .. } => {}
                }
            }
        }

        if should_rebuild_snapshot || slide_analyses > 0 {
            core.rebuild_snapshot();
        }

        let notion_write_records = core
            .snapshot
            .person_page
            .profiles
            .iter()
            .into_iter()
            .filter_map(|person| {
                let frontend = person.frontend_profile.clone()?;
                let profile = frontend.profile;
                let entity_id = profile
                    .email
                    .as_deref()
                    .or(profile.generated_email.as_deref())
                    .or(profile.source_document_id.as_deref())?
                    .to_string();
                Some((
                    entity_id.clone(),
                    crate::adapter::writeback::traits::WriteRecord {
                        entity_id,
                        title: if profile.name.trim().is_empty() {
                            frontend
                                .source_document_id
                                .rsplit_once("#slide:")
                                .map(|(_, slide_id)| slide_id.to_string())
                                .unwrap_or_else(|| "Untitled Slide".to_string())
                        } else {
                            profile.name.clone()
                        },
                        payload: serde_json::to_value(profile).ok()?,
                        external_id: None,
                    },
                ))
            })
            .collect::<HashMap<_, _>>()
            .into_values()
            .collect::<Vec<_>>();

        drop(core);

        if let Some(notion) = &self.notion_client {
            for mut write_record in notion_write_records {
                write_record.external_id = notion.find_existing(&write_record.entity_id)?;
                notion.write_record(&write_record)?;
                notion_synced += 1;
            }
        }

        Ok(SyncReport {
            slack_ingested,
            google_ingested,
            slide_analyses,
            notion_synced,
            duplicates,
            last_sync_at,
        })
    }

    pub fn persons_response(
        &self,
        read_mode: Option<&str>,
        pin: Option<&str>,
        pagination: &PaginationParams,
    ) -> Result<ResponseEnvelope<serde_json::Value>, SelfHostError> {
        let core = self.core_lock()?;
        let mode = self.resolve_read_mode(&core.catalog, "proj:person-page", read_mode, pin)?;
        self.authorize_read(EntityRef::new("projection:person-page"))?;

        let mut list: Vec<PersonListItem> = core
            .snapshot
            .person_page
            .profiles
            .iter()
            .filter_map(|profile| {
                let activity = core
                    .snapshot
                    .person_page
                    .activities
                    .iter()
                    .find(|activity| activity.person_id == profile.person_id)?;
                Some(PersonPageProjector::to_list_item(profile, activity))
            })
            .collect();
        list.sort_by(|left, right| right.last_activity.cmp(&left.last_activity));

        let (page, total) = paginate(&list, pagination);
        let payload = serde_json::to_value(PaginatedResponse::from_slice(page, total, pagination))?;

        Ok(ResponseEnvelope {
            data: self.apply_filter(payload),
            projection_metadata: self.projection_metadata(&core.catalog, "proj:person-page", mode, core.snapshot.built_at)?,
        })
    }

    pub fn person_detail_response(
        &self,
        person_id: &str,
        read_mode: Option<&str>,
        pin: Option<&str>,
    ) -> Result<ResponseEnvelope<serde_json::Value>, SelfHostError> {
        let core = self.core_lock()?;
        let mode = self.resolve_read_mode(&core.catalog, "proj:person-page", read_mode, pin)?;
        self.authorize_read(EntityRef::new(person_id.to_string()))?;

        let profile = core
            .snapshot
            .person_page
            .profiles
            .iter()
            .find(|profile| profile.person_id.as_str() == person_id)
            .ok_or_else(|| SelfHostError::NotFound(person_id.to_string()))?;
        let slides: Vec<_> = core
            .snapshot
            .person_page
            .slides
            .iter()
            .filter(|slide| slide.person_id == profile.person_id)
            .cloned()
            .collect();
        let messages: Vec<_> = core
            .snapshot
            .person_page
            .messages
            .iter()
            .filter(|message| message.person_id == profile.person_id)
            .cloned()
            .collect();
        let activity = core
            .snapshot
            .person_page
            .activities
            .iter()
            .find(|activity| activity.person_id == profile.person_id)
            .ok_or_else(|| SelfHostError::NotFound(format!("activity for {person_id}")))?;

        let detail: PersonDetailResponse = PersonPageProjector::to_detail(profile, &slides, &messages, activity);
        Ok(ResponseEnvelope {
            data: self.apply_filter(serde_json::to_value(detail)?),
            projection_metadata: self.projection_metadata(&core.catalog, "proj:person-page", mode, core.snapshot.built_at)?,
        })
    }

    pub fn person_slides_response(
        &self,
        person_id: &str,
        read_mode: Option<&str>,
        pin: Option<&str>,
    ) -> Result<ResponseEnvelope<serde_json::Value>, SelfHostError> {
        let core = self.core_lock()?;
        let mode = self.resolve_read_mode(&core.catalog, "proj:person-page", read_mode, pin)?;
        self.authorize_read(EntityRef::new(person_id.to_string()))?;
        let slides: Vec<_> = core
            .snapshot
            .person_page
            .slides
            .iter()
            .filter(|slide| slide.person_id.as_str() == person_id)
            .cloned()
            .collect();

        Ok(ResponseEnvelope {
            data: self.apply_filter(serde_json::to_value(slides)?),
            projection_metadata: self.projection_metadata(&core.catalog, "proj:person-page", mode, core.snapshot.built_at)?,
        })
    }

    pub fn person_messages_response(
        &self,
        person_id: &str,
        read_mode: Option<&str>,
        pin: Option<&str>,
    ) -> Result<ResponseEnvelope<serde_json::Value>, SelfHostError> {
        let core = self.core_lock()?;
        let mode = self.resolve_read_mode(&core.catalog, "proj:person-page", read_mode, pin)?;
        self.authorize_read(EntityRef::new(person_id.to_string()))?;
        let messages: Vec<_> = core
            .snapshot
            .person_page
            .messages
            .iter()
            .filter(|message| message.person_id.as_str() == person_id)
            .cloned()
            .collect();

        Ok(ResponseEnvelope {
            data: self.apply_filter(serde_json::to_value(messages)?),
            projection_metadata: self.projection_metadata(&core.catalog, "proj:person-page", mode, core.snapshot.built_at)?,
        })
    }

    pub fn person_timeline_response(
        &self,
        person_id: &str,
        read_mode: Option<&str>,
        pin: Option<&str>,
    ) -> Result<ResponseEnvelope<serde_json::Value>, SelfHostError> {
        let core = self.core_lock()?;
        let mode = self.resolve_read_mode(&core.catalog, "proj:person-page", read_mode, pin)?;
        self.authorize_read(EntityRef::new(person_id.to_string()))?;
        let mut events = Vec::new();

        for slide in core
            .snapshot
            .person_page
            .slides
            .iter()
            .filter(|slide| slide.person_id.as_str() == person_id)
        {
            if let Some(ts) = slide.last_modified {
                events.push(TimelineEvent {
                    event_type: "slide".into(),
                    document_id: Some(slide.document_id.clone()),
                    channel: None,
                    title: Some(slide.title.clone()),
                    text: None,
                    ts,
                });
            }
        }

        for message in core
            .snapshot
            .person_page
            .messages
            .iter()
            .filter(|message| message.person_id.as_str() == person_id)
        {
            events.push(TimelineEvent {
                event_type: "message".into(),
                document_id: None,
                channel: Some(message.channel.clone()),
                title: None,
                text: Some(message.text.clone()),
                ts: message.ts,
            });
        }

        events.sort_by(|left, right| right.ts.cmp(&left.ts));

        Ok(ResponseEnvelope {
            data: self.apply_filter(serde_json::to_value(events)?),
            projection_metadata: self.projection_metadata(&core.catalog, "proj:person-page", mode, core.snapshot.built_at)?,
        })
    }

    pub fn health(&self) -> Result<HealthResponse, SelfHostError> {
        let core = self.core_lock()?;
        Ok(HealthResponse::from_catalog(&core.catalog, env!("CARGO_PKG_VERSION")))
    }

    fn authorize_read(&self, target: EntityRef) -> Result<(), SelfHostError> {
        let outcome = PolicyEngine::evaluate(&PolicyRequest {
            actor: ActorRef::new("actor:self-host"),
            role: Role::Researcher,
            operation: Operation::Read { target },
            data_scope: AccessScope::Internal,
            consent_status: ConsentStatus::Unrestricted,
            environment: Environment::Production,
        });

        match outcome {
            PolicyOutcome::Allow => Ok(()),
            PolicyOutcome::Deny { reason } => Err(SelfHostError::Policy(reason.message)),
            PolicyOutcome::RequireReview { route } => Err(SelfHostError::Policy(route.reason)),
        }
    }

    fn projection_metadata(
        &self,
        catalog: &ProjectionCatalog,
        projection_id: &str,
        read_mode: ReadMode,
        built_at: DateTime<Utc>,
    ) -> Result<ProjectionMetadata, SelfHostError> {
        let projection_id = ProjectionRef::new(projection_id);
        let entry = catalog
            .get(&projection_id)
            .ok_or_else(|| SelfHostError::NotFound(projection_id.to_string()))?;
        Ok(ProjectionMetadata {
            projection_id,
            version: entry.spec.version.clone(),
            built_at,
            read_mode,
            stale: false,
            lineage_ref: None,
        })
    }

    fn apply_filter(&self, payload: serde_json::Value) -> serde_json::Value {
        FilteringGate::filter(&payload, AccessScope::Internal, &restricted_fields()).payload
    }

    fn resolve_read_mode(
        &self,
        catalog: &ProjectionCatalog,
        projection_id: &str,
        read_mode: Option<&str>,
        pin: Option<&str>,
    ) -> Result<ReadMode, SelfHostError> {
        let spec = &catalog
            .get(&ProjectionRef::new(projection_id))
            .ok_or_else(|| SelfHostError::NotFound(projection_id.to_string()))?
            .spec;
        ReadModeResolver::resolve(spec, read_mode, pin)
            .map_err(|err: ReadModeError| SelfHostError::ReadMode(err.to_string()))
    }

    fn ingest_draft(&self, draft: ObservationDraft) -> Result<IngestResult, SelfHostError> {
        let mut core = self.core_lock()?;
        let result = core.ingest(draft);

        if let IngestResult::Ingested { id, .. } = &result {
            let observation = core
                .lake
                .get(id)
                .cloned()
                .ok_or_else(|| SelfHostError::Ingestion(format!("observation {id} missing after append")))?;
            if let Err(err) = self.persistence_lock()?.persist_observation(&observation) {
                core.lake.rollback_last_append(id);
                return Err(SelfHostError::Persistence(err));
            }
        }

        match &result {
            IngestResult::Rejected { message, .. } => Err(SelfHostError::Ingestion(message.clone())),
            IngestResult::Quarantined { ticket } => Err(SelfHostError::Ingestion(ticket.reason.clone())),
            _ => Ok(result),
        }
    }

    fn store_blob(&self, data: &[u8]) -> Result<BlobRef, SelfHostError> {
        let mut core = self.core_lock()?;
        let blob_ref = core.blobs.put(data);
        self.persistence_lock()?.persist_blob(data)?;
        Ok(blob_ref)
    }

    fn ingest_slack_message(
        &self,
        slack_adapter: &SlackAdapter<HttpSlackClient>,
        file_client: &HttpSlackClient,
        channel_id: &str,
        mut message: crate::adapter::slack::client::SlackMessage,
        latest_ts: &mut Option<String>,
    ) -> Result<IngestResult, SelfHostError> {
        message.channel_id = channel_id.to_string();
        for file in &mut message.files {
            if file.blob_ref.is_none() {
                let data = file_client.file_download(file)?;
                let blob_ref = self.store_blob(&data)?;
                file.blob_ref = Some(blob_ref.as_str().to_string());
            }
        }
        if latest_ts
            .as_ref()
            .map(|current| slack_ts_value(&message.ts) > slack_ts_value(current))
            .unwrap_or(true)
        {
            *latest_ts = Some(message.ts.clone());
        }
        self.ingest_draft(slack_adapter.map_message(&message))
    }

    fn extract_student_profile(
        &self,
        observation: &Observation,
        blobs: &BlobStore,
    ) -> Option<crate::slide_analysis::types::StudentProfile> {
        if let Some(analyzer) = &self.slide_analyzer {
            match analyzer.extract_profile(observation, blobs) {
                Ok(Some(profile)) => return Some(profile),
                Ok(None) => {}
                Err(err) => eprintln!(
                    "slide ai analysis failed for {}: {err}; falling back to heuristic profile",
                    observation.id
                ),
            }
        }

        heuristic_profile(observation)
    }

    fn extract_student_profile_from_png(
        &self,
        image: &[u8],
        observation: &Observation,
        canonical_uri: &str,
    ) -> Option<crate::slide_analysis::types::StudentProfile> {
        let title = observation
            .payload
            .get("title")
            .and_then(|value| value.as_str())
            .unwrap_or("Unknown");

        if let Some(analyzer) = &self.slide_analyzer {
            match analyzer.extract_profile_from_png(image, title, canonical_uri) {
                Ok(Some(profile)) => return Some(profile),
                Ok(None) => {}
                Err(err) => eprintln!(
                    "slide ai analysis failed for {}: {err}; falling back to heuristic profile",
                    observation.id
                ),
            }
        }

        None
    }

    fn core_lock(&self) -> Result<std::sync::MutexGuard<'_, AppCore>, SelfHostError> {
        self.core.lock().map_err(|_| SelfHostError::LockPoisoned)
    }

    fn persistence_lock(&self) -> Result<std::sync::MutexGuard<'_, SqlitePersistence>, SelfHostError> {
        self.persistence.lock().map_err(|_| SelfHostError::LockPoisoned)
    }

    fn slack_adapter_config(&self) -> AdapterConfig {
        AdapterConfig {
            observer_id: ObserverRef::new("obs:slack-crawler"),
            source_system_id: SourceSystemRef::new("sys:slack"),
            adapter_version: SemVer::new("1.0.0"),
            authority_model: AuthorityModel::LakeAuthoritative,
            capture_model: CaptureModel::Event,
            schemas: vec![
                SchemaRef::new("schema:slack-message"),
                SchemaRef::new("schema:slack-channel-snapshot"),
                SchemaRef::new("schema:observer-heartbeat"),
            ],
            schema_bindings: vec![SchemaBinding {
                schema: SchemaRef::new("schema:slack-message"),
                versions: ">=1.0.0 <2.0.0".into(),
            }],
            poll_interval: self.config.poll_interval,
            heartbeat_interval: self.config.poll_interval,
            rate_limit: RateLimitConfig {
                requests_per_second: 50,
                burst: 10,
            },
            retry: RetryConfig {
                max_retries: 3,
                backoff: BackoffStrategy::Exponential,
                max_wait: self.config.poll_interval,
            },
            credential_ref: "env:DOKP_SLACK_BOT_TOKEN".into(),
        }
    }

    fn google_adapter_config(&self) -> AdapterConfig {
        AdapterConfig {
            observer_id: ObserverRef::new("obs:gslides-crawler"),
            source_system_id: SourceSystemRef::new("sys:google-slides"),
            adapter_version: SemVer::new("1.0.0"),
            authority_model: AuthorityModel::SourceAuthoritative,
            capture_model: CaptureModel::Snapshot,
            schemas: vec![
                SchemaRef::new("schema:workspace-object-snapshot"),
                SchemaRef::new("schema:observer-heartbeat"),
            ],
            schema_bindings: vec![SchemaBinding {
                schema: SchemaRef::new("schema:workspace-object-snapshot"),
                versions: ">=1.0.0 <2.0.0".into(),
            }],
            poll_interval: self.config.poll_interval,
            heartbeat_interval: self.config.poll_interval,
            rate_limit: RateLimitConfig {
                requests_per_second: 10,
                burst: 5,
            },
            retry: RetryConfig {
                max_retries: 3,
                backoff: BackoffStrategy::Exponential,
                max_wait: self.config.poll_interval,
            },
            credential_ref: "env:DOKP_GOOGLE_ACCESS_TOKEN".into(),
        }
    }
}

fn revisions_after_cursor(
    revisions: Vec<crate::adapter::gslides::client::SlideRevision>,
    cursor: Option<&str>,
    reset: bool,
) -> Vec<crate::adapter::gslides::client::SlideRevision> {
    if cursor.is_none() || reset {
        return revisions;
    }

    let cursor = cursor.unwrap();
    let mut found = false;
    revisions
        .into_iter()
        .filter(|revision| {
            if found {
                true
            } else if revision.revision_id == cursor {
                found = true;
                false
            } else {
                false
            }
        })
        .collect()
}

fn latest_revision_to_capture(
    revisions: &[crate::adapter::gslides::client::SlideRevision],
) -> Option<&crate::adapter::gslides::client::SlideRevision> {
    // The Google APIs used here only let us fetch the current presentation state,
    // so capturing anything older than the newest unseen revision would falsely
    // attach latest content to historical revision IDs.
    revisions.last()
}

fn thread_root_ts(
    message: &crate::adapter::slack::client::SlackMessage,
) -> Option<&str> {
    if message.reply_count == 0 {
        return None;
    }

    Some(message.thread_ts.as_deref().unwrap_or(message.ts.as_str()))
}

fn non_empty_state(value: Option<String>) -> Option<String> {
    value.filter(|raw| !raw.trim().is_empty())
}

fn restricted_fields() -> Vec<RestrictedFieldSpec> {
    vec![RestrictedFieldSpec {
        field_path: "identities".into(),
        level: AccessScope::Restricted,
        mask_strategy: MaskStrategy::Exclude,
    }]
}

fn slack_ts_value(value: &str) -> f64 {
    value.parse::<f64>().unwrap_or(0.0)
}

fn heuristic_profile(observation: &Observation) -> Option<crate::slide_analysis::types::StudentProfile> {
    let title = observation
        .payload
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown");
    Some(crate::slide_analysis::types::StudentProfile {
        email: None,
        generated_email: None,
        name: title.to_string(),
        bio_text: None,
        profile_pic: None,
        gallery_images: vec![],
        properties: Default::default(),
        attributes: vec![],
        source_slide_object_id: None,
        source_document_id: None,
        source_canonical_uri: None,
        thumbnail_blob_ref: None,
        thumbnail_url: None,
        companion_to_slide_object_id: None,
    })
}

fn analysis_record_needs_refresh(
    record: &crate::domain::SupplementalRecord,
    analysis_model: &str,
) -> bool {
    !analysis_record_is_rich(record) || record.model_version.as_deref() != Some(analysis_model)
}

fn should_merge_companion_slide(
    primary: &crate::slide_analysis::types::StudentProfile,
    companion: &crate::slide_analysis::types::StudentProfile,
    observation: &Observation,
) -> bool {
    if !profile_has_content(companion) {
        return false;
    }

    if companion
        .email
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return false;
    }

    let deck_title = observation
        .payload
        .get("title")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let primary_name = normalize_profile_name(&primary.name);
    let companion_name = normalize_profile_name(&companion.name);

    companion_name.is_empty()
        || companion_name == normalize_profile_name(deck_title)
        || (!primary_name.is_empty() && companion_name == primary_name)
}

fn profile_has_content(profile: &crate::slide_analysis::types::StudentProfile) -> bool {
    profile.bio_text.as_ref().is_some_and(|text| !text.trim().is_empty())
        || profile.profile_pic.is_some()
        || !profile.gallery_images.is_empty()
        || !profile.attributes.is_empty()
        || profile.properties.nickname.is_some()
        || profile.properties.birthplace.is_some()
        || profile.properties.dob.is_some()
        || profile.properties.major.is_some()
        || profile.properties.affiliation.is_some()
        || profile.properties.mbti.is_some()
        || profile.properties.sns.is_some()
        || !profile.properties.hobbies.is_empty()
        || !profile.properties.interests.is_empty()
        || !profile.properties.likes.is_empty()
        || profile.properties.dislikes.is_some()
        || !profile.properties.hashtags.is_empty()
        || profile.properties.new_challenges.is_some()
        || profile.properties.ask_me_about.is_some()
        || profile.properties.turning_point.is_some()
        || profile.properties.btw.is_some()
        || profile.properties.message.is_some()
        || profile.thumbnail_url.is_some()
}

fn normalize_profile_name(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>()
        .to_lowercase()
}

fn merge_companion_profile(
    primary: &mut crate::slide_analysis::types::StudentProfile,
    companion: &crate::slide_analysis::types::StudentProfile,
) {
    if let Some(companion_thumbnail_url) = &companion.thumbnail_url {
        let description = companion
            .bio_text
            .clone()
            .or_else(|| companion.profile_pic.as_ref().and_then(|pic| pic.description.clone()))
            .or_else(|| Some("Continuation slide".to_string()));
        primary.gallery_images.push(crate::slide_analysis::types::GalleryImage {
            coordinates: None,
            description,
            url: Some(companion_thumbnail_url.clone()),
        });
    }

    primary.gallery_images.extend(companion.gallery_images.clone());

    if let Some(companion_bio) = companion
        .bio_text
        .as_ref()
        .map(|text| text.trim())
        .filter(|text| !text.is_empty())
    {
        match primary.bio_text.as_mut() {
            Some(primary_bio) if !primary_bio.contains(companion_bio) => {
                primary_bio.push_str("\n\n");
                primary_bio.push_str(companion_bio);
            }
            None => primary.bio_text = Some(companion_bio.to_string()),
            _ => {}
        }
    }

    if primary.profile_pic.is_none() {
        primary.profile_pic = companion.profile_pic.clone();
    }

    merge_optional_field(&mut primary.properties.nickname, &companion.properties.nickname);
    merge_optional_field(&mut primary.properties.birthplace, &companion.properties.birthplace);
    merge_optional_field(&mut primary.properties.dob, &companion.properties.dob);
    merge_optional_field(&mut primary.properties.major, &companion.properties.major);
    merge_optional_field(&mut primary.properties.affiliation, &companion.properties.affiliation);
    merge_optional_field(&mut primary.properties.mbti, &companion.properties.mbti);
    merge_optional_field(&mut primary.properties.sns, &companion.properties.sns);
    merge_optional_field(&mut primary.properties.dislikes, &companion.properties.dislikes);
    merge_optional_field(&mut primary.properties.new_challenges, &companion.properties.new_challenges);
    merge_optional_field(&mut primary.properties.ask_me_about, &companion.properties.ask_me_about);
    merge_optional_field(&mut primary.properties.turning_point, &companion.properties.turning_point);
    merge_optional_field(&mut primary.properties.btw, &companion.properties.btw);
    merge_optional_field(&mut primary.properties.message, &companion.properties.message);

    append_distinct_strings(&mut primary.properties.hobbies, &companion.properties.hobbies);
    append_distinct_strings(&mut primary.properties.interests, &companion.properties.interests);
    append_distinct_strings(&mut primary.properties.likes, &companion.properties.likes);
    append_distinct_strings(&mut primary.properties.hashtags, &companion.properties.hashtags);
    append_distinct_strings(&mut primary.attributes, &companion.attributes);
}

fn merge_optional_field(target: &mut Option<String>, source: &Option<String>) {
    if target.as_ref().is_some_and(|value| !value.trim().is_empty()) {
        return;
    }
    *target = source.clone();
}

fn append_distinct_strings(target: &mut Vec<String>, source: &[String]) {
    for value in source {
        if !target.contains(value) {
            target.push(value.clone());
        }
    }
}

fn ensure_profile_identifier(
    profile: &mut crate::slide_analysis::types::StudentProfile,
    slide_object_id: &str,
) {
    if profile
        .email
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
        || profile
            .generated_email
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
    {
        return;
    }

    let fallback = slide_object_id
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch.to_ascii_lowercase() } else { '-' })
        .collect::<String>();
    profile.generated_email = Some(format!("slide-{fallback}@hlab.college"));
}

fn find_slide_analysis_record<'a>(
    records: &'a [crate::domain::SupplementalRecord],
    presentation_id: &str,
    slide_object_id: &str,
) -> Option<&'a crate::domain::SupplementalRecord> {
    records.iter().find(|record| {
        if record.kind != "slide-analysis" {
            return false;
        }
        let Ok(profile) = serde_json::from_value::<crate::slide_analysis::types::StudentProfile>(record.payload.clone()) else {
            return false;
        };
        profile.source_document_id.as_deref() == Some(&format!(
            "document:gslides:{presentation_id}#slide:{slide_object_id}"
        )) || profile.source_slide_object_id.as_deref() == Some(slide_object_id)
    })
}

fn analysis_record_is_rich(record: &crate::domain::SupplementalRecord) -> bool {
    let Ok(profile) = serde_json::from_value::<crate::slide_analysis::types::StudentProfile>(record.payload.clone()) else {
        return false;
    };

    profile.bio_text.as_ref().is_some_and(|text| !text.trim().is_empty())
        || profile.profile_pic.is_some()
        || !profile.gallery_images.is_empty()
        || !profile.attributes.is_empty()
        || profile.properties.nickname.is_some()
        || profile.properties.birthplace.is_some()
        || profile.properties.dob.is_some()
        || profile.properties.major.is_some()
        || profile.properties.affiliation.is_some()
        || profile.properties.mbti.is_some()
        || profile.properties.sns.is_some()
        || !profile.properties.hobbies.is_empty()
        || !profile.properties.interests.is_empty()
        || !profile.properties.likes.is_empty()
        || profile.properties.dislikes.is_some()
        || !profile.properties.hashtags.is_empty()
        || profile.properties.new_challenges.is_some()
        || profile.properties.ask_me_about.is_some()
        || profile.properties.turning_point.is_some()
        || profile.properties.btw.is_some()
        || profile.properties.message.is_some()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use crate::adapter::slack::client::{SlackMessage, SlackMessageType};
    use crate::adapter::traits::ObservationDraft;
    use crate::domain::supplemental::InputAnchorSet;
    use crate::adapter::gslides::client::SlideRevision;
    use chrono::Utc;

    use super::{AppCore, AppService, SelfHostError, latest_revision_to_capture, non_empty_state, thread_root_ts};
    use crate::domain::{
        ActorRef, AuthorityModel, CaptureModel, EntityRef, IdempotencyKey, Mutability,
        Observation, ObserverRef, SchemaRef, SemVer, SupplementalId, SupplementalRecord,
    };
    use crate::self_host::config::{GoogleConfig, SelfHostConfig, SlackConfig};
    use crate::self_host::google::HttpGoogleSlidesClient;
    use crate::self_host::persistence::SqlitePersistence;
    use crate::self_host::slack::HttpSlackClient;

    #[test]
    fn non_empty_state_filters_blank_values() {
        assert_eq!(non_empty_state(None), None);
        assert_eq!(non_empty_state(Some(String::new())), None);
        assert_eq!(non_empty_state(Some("   ".to_string())), None);
        assert_eq!(
            non_empty_state(Some("1234567890.123456".to_string())).as_deref(),
            Some("1234567890.123456")
        );
    }

    #[test]
    fn app_core_new_rejects_duplicate_persisted_observations() {
        fn observation(id: &str, key: &str) -> Observation {
            Observation {
                id: Observation::new_id(),
                schema: SchemaRef::new("schema:test"),
                schema_version: SemVer::new("1.0.0"),
                observer: ObserverRef::new("obs:test"),
                source_system: None,
                actor: None,
                authority_model: AuthorityModel::LakeAuthoritative,
                capture_model: CaptureModel::Event,
                subject: EntityRef::new(format!("entity:{id}")),
                target: None,
                payload: serde_json::json!({ "id": id }),
                attachments: vec![],
                published: Utc::now(),
                recorded_at: Utc::now(),
                consent: None,
                idempotency_key: Some(IdempotencyKey::new(key)),
                meta: serde_json::json!({}),
            }
        }

        let observations = vec![
            observation("one", "dup-key"),
            observation("two", "dup-key"),
        ];

        let err = AppCore::new(observations, vec![], vec![]).unwrap_err();
        assert!(matches!(err, SelfHostError::Ingestion(_)));
    }

    #[test]
    fn latest_revision_to_capture_prefers_newest_revision() {
        let revisions = vec![
            SlideRevision {
                presentation_id: "pres-1".into(),
                revision_id: "rev-1".into(),
                modified_time: chrono::DateTime::parse_from_rfc3339("2026-03-24T10:00:00Z")
                    .unwrap()
                    .to_utc(),
                last_modifying_user: None,
            },
            SlideRevision {
                presentation_id: "pres-1".into(),
                revision_id: "rev-2".into(),
                modified_time: chrono::DateTime::parse_from_rfc3339("2026-03-24T11:00:00Z")
                    .unwrap()
                    .to_utc(),
                last_modifying_user: None,
            },
        ];

        assert_eq!(
            latest_revision_to_capture(&revisions)
                .map(|revision| revision.revision_id.as_str()),
            Some("rev-2")
        );
    }

    fn test_config(db: PathBuf, blobs: PathBuf) -> SelfHostConfig {
        SelfHostConfig {
            bind_addr: "127.0.0.1:0".into(),
            database_path: db,
            blob_dir: blobs,
            poll_interval: std::time::Duration::from_secs(300),
            slack: SlackConfig {
                bot_token: "xoxb-test-token".into(),
                thread_token: None,
                channel_ids: vec!["C01ABC".into()],
            },
            google: GoogleConfig {
                access_token: Some("ya29.test-token".into()),
                client_id: None,
                client_secret: None,
                refresh_token: None,
                presentation_ids: vec!["pres123".into()],
            },
            slide_analysis_limit: 10,
            slide_ai: None,
            notion: None,
        }
    }

    #[test]
    fn thread_root_ts_returns_parent_thread_identifier() {
        let message = SlackMessage {
            channel_id: "C01ABC".into(),
            channel_name: "general".into(),
            ts: "1234567890.123456".into(),
            thread_ts: None,
            user_id: "U1".into(),
            user_name: "alice".into(),
            email: None,
            text: "hello".into(),
            message_type: SlackMessageType::Message,
            edited: None,
            reactions: vec![],
            files: vec![],
            reply_count: 2,
            reply_users_count: 1,
        };

        assert_eq!(thread_root_ts(&message), Some("1234567890.123456"));
    }

    #[test]
    fn ingest_draft_rolls_back_lake_when_persistence_fails() {
        let root = std::env::temp_dir().join(format!("dokp-self-host-test-{}", uuid::Uuid::now_v7()));
        let db = root.join("dokp.sqlite3");
        let blobs = root.join("blobs");
        let persistence = SqlitePersistence::open(&db, &blobs).unwrap();
        let persisted_observation = Observation {
            id: Observation::new_id(),
            schema: SchemaRef::new("schema:slack-message"),
            schema_version: SemVer::new("1.0.0"),
            observer: ObserverRef::new("obs:slack-crawler"),
            source_system: Some(crate::domain::SourceSystemRef::new("sys:slack")),
            actor: None,
            authority_model: AuthorityModel::LakeAuthoritative,
            capture_model: CaptureModel::Event,
            subject: EntityRef::new("message:slack:existing"),
            target: None,
            payload: serde_json::json!({"text": "persisted"}),
            attachments: vec![],
            published: Utc::now(),
            recorded_at: Utc::now(),
            consent: None,
            idempotency_key: Some(IdempotencyKey::new("slack:C01ABC:dup-ts")),
            meta: serde_json::json!({}),
        };
        persistence.persist_observation(&persisted_observation).unwrap();

        let config = test_config(db.clone(), blobs.clone());
        let service = AppService {
            core: Arc::new(Mutex::new(AppCore::new(vec![], vec![], vec![]).unwrap())),
            persistence: Arc::new(Mutex::new(persistence)),
            config: Arc::new(config.clone()),
            slack_client: HttpSlackClient::new(config.slack.bot_token.clone()).unwrap(),
            slack_replies_client: HttpSlackClient::new(config.slack.bot_token.clone()).unwrap(),
            google_client: HttpGoogleSlidesClient::new(&config.google).unwrap(),
            slide_analyzer: None,
            notion_client: None,
        };

        let draft = ObservationDraft {
            schema: SchemaRef::new("schema:slack-message"),
            schema_version: SemVer::new("1.0.0"),
            observer: ObserverRef::new("obs:slack-crawler"),
            source_system: Some(crate::domain::SourceSystemRef::new("sys:slack")),
            authority_model: AuthorityModel::LakeAuthoritative,
            capture_model: CaptureModel::Event,
            subject: EntityRef::new("message:slack:new"),
            target: None,
            payload: serde_json::json!({
                "channel_id": "C01ABC",
                "channel_name": "general",
                "ts": "dup-ts",
                "user_id": "U1",
                "user_name": "alice",
                "text": "new"
            }),
            attachments: vec![],
            published: Utc::now(),
            idempotency_key: IdempotencyKey::new("slack:C01ABC:dup-ts"),
            meta: serde_json::json!({}),
        };

        let err = service.ingest_draft(draft).unwrap_err();
        assert!(matches!(err, SelfHostError::Persistence(_)));
        assert_eq!(service.core_lock().unwrap().lake.len(), 0);
        assert_eq!(service.persistence_lock().unwrap().load_observations().unwrap().len(), 1);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn app_core_restores_persisted_slide_analysis_supplemental() {
        let observation = Observation {
            id: Observation::new_id(),
            schema: SchemaRef::new("schema:workspace-object-snapshot"),
            schema_version: SemVer::new("1.0.0"),
            observer: ObserverRef::new("obs:gslides-crawler"),
            source_system: Some(crate::domain::SourceSystemRef::new("sys:google-slides")),
            actor: None,
            authority_model: AuthorityModel::SourceAuthoritative,
            capture_model: CaptureModel::Snapshot,
            subject: EntityRef::new("document:gslides:pres123"),
            target: None,
            payload: serde_json::json!({
                "title": "自己紹介",
                "artifact": { "sourceObjectId": "pres123" },
                "relations": {
                    "owner": "tanaka@example.jp",
                    "editors": ["tanaka@example.jp"]
                }
            }),
            attachments: vec![],
            published: Utc::now(),
            recorded_at: Utc::now(),
            consent: None,
            idempotency_key: Some(IdempotencyKey::new("gslides:pres123:rev:r1")),
            meta: serde_json::json!({}),
        };
        let supplemental = SupplementalRecord {
            id: SupplementalId::new("sup:slide-analysis:pres123:slide-1"),
            kind: "slide-analysis".into(),
            derived_from: InputAnchorSet {
                observations: vec![observation.id.clone()],
                blobs: vec![],
                supplementals: vec![],
            },
            payload: serde_json::json!({
                "name": "田中太郎",
                "bio_text": "私は田中太郎です",
                "source_slide_object_id": "slide-1",
                "source_document_id": "document:gslides:pres123#slide:slide-1"
            }),
            created_by: ActorRef::new("actor:test"),
            created_at: Utc::now(),
            mutability: Mutability::ManagedCache,
            record_version: Some("1".into()),
            model_version: Some("fixture".into()),
            consent_metadata: None,
            lineage: None,
        };

        let core = AppCore::new(vec![observation], vec![], vec![supplemental]).unwrap();
        assert_eq!(
            core.snapshot.person_page.profiles[0].self_intro_text.as_deref(),
            Some("私は田中太郎です")
        );
    }
}
