//! Chton: the IO materialization layer.
//!
//! Chton lands the tagma coordinate space onto physical origins. The crate is
//! strictly layered:
//!
//! - the origin layer provides byte-level bindings (memory, file; signal,
//!   network, and GPU origins are future contexts),
//! - the binding layer adapts coordinate paths to origin offsets,
//! - the protocol layer defines materialization protocols, with key-value as
//!   the first protocol.
//!
//! A protocol is origin-agnostic and an origin is protocol-agnostic. Storage
//! is what a disk origin does, transmission is what a signal origin does, and
//! a protocol materializes over either.

pub mod binding;
pub mod origin;
pub mod protocol;
