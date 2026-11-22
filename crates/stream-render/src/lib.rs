//! The render worker: claims a recorded lesson from the site, draws it natively (Skia, with ffmpeg
//! as a child process for decoding and encoding), uploads the MP4 and reports it.

pub mod card;
pub mod config;
pub mod engine;
pub mod heartbeat;
pub mod job;
pub mod media;
pub mod native;
pub mod recording;
pub mod upload;
pub mod worker;
