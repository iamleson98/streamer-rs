//! What the stream workloads share: the worker routes' types and client, the sealed stream key,
//! redaction of secrets from everything a workload logs or sends, the destination map and the
//! failure codes.

pub mod api;
pub mod codes;
pub mod destination;
pub mod key;
pub mod probe;
pub mod redact;
