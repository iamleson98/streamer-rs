//! The stream runner: polls the site for the next stream due, checks the rendered file and copies it
//! to the platform's ingest with ffmpeg, heartbeating while it does.

pub mod config;
pub mod ffmpeg;
pub mod heartbeat;
pub mod probe;
pub mod runner;
