//! Notion SaaS Write-back Adapter
//!
//! Implements the `SaaSWriteAdapter` trait for Notion databases.
//! Ported from the Google Apps Script `NotionService.js` in skcollege_dictionary,
//! adapted to the DOKP write-back protocol.

pub mod client;

pub use client::*;
