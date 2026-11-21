//! The render worker: claims a recorded lesson from the site, renders it in a headless browser on
//! the site's render page, remuxes the output, uploads the MP4 and reports it.

pub mod browser;
pub mod card;
pub mod chrome;
pub mod chromium;
pub mod config;
pub mod engine;
pub mod heartbeat;
pub mod job;
pub mod media;
pub mod proxy;
pub mod server;
pub mod sink;
pub mod upload;
pub mod worker;
