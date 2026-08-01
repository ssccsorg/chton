//! Protocol layer: materialization protocols over origins.
//!
//! A protocol is origin-agnostic: it observes only the origin surface and
//! defines the semantics of materialization. Key-value is the first
//! protocol; log, blob, and stream protocols are later candidates.

pub mod kv;
