//! M06: DAG Propagation
//!
//! Watermark management, incremental propagation scheduler,
//! topological order execution, and health status tracking.

pub mod scheduler;
pub mod watermark;

pub use scheduler::PropagationScheduler;
pub use watermark::{WatermarkState, WatermarkStore};
