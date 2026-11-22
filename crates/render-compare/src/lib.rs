//! `render-compare`: one recording rendered by the Chromium engine and by the native engine, and a
//! report that says how the two videos differ. See the crate's README.

pub mod analyze;
pub mod cost;
pub mod metrics;
pub mod report;
pub mod rig;
pub mod timeline;
