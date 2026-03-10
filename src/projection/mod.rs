//! M05: Projection Engine
//!
//! Projection spec model, source declaration validation, lineage manifest,
//! catalog with DAG acyclicity check, and build runner abstraction.

pub mod catalog;
pub mod lineage;
pub mod spec;
pub mod runner;

pub use catalog::ProjectionCatalog;
pub use lineage::LineageManifest;
pub use spec::{
    BuildSpec, GapAction, OutputSpec, ProjectionSpec, ReadModePolicy,
    ReconciliationPolicy, SourceDecl, SourceRef,
};
pub use runner::{BuildResult, BuildStatus, ProjectionRunner};
