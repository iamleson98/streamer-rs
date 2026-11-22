//! A stand-in for the site, for testing and comparing the render engines: the recording reads
//! (`manifest`, `files/{n}` redirecting to a ranged storage, deck page pictures) and the render
//! lease's heartbeat, served from a directory; and synthetic recordings made with ffmpeg, so a
//! test or a report needs no real lesson. Nothing here is a secret or a real person's.

pub mod recording;
pub mod serve;
pub mod synth;

pub use recording::{RecFile, Recording};
pub use serve::{Request, Server, serve};
pub use synth::{Scenario, build};
