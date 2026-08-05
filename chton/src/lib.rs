//! Chton: the IO materialization fabric: matrix router and transformation
//! implementations for coordinate spaces over physical media.
//!
//! Chton lands the tagma coordinate space onto origins outside memory. The
//! surface is a matrix: space types, origins, and build targets are axes, and
//! a cell is a materialization path that is also the IO path. Consumers
//! address paths by coordinates; chton routes to the implementation. The
//! crate is strictly layered:
//!
//! - the origin layer provides byte-level bindings (file; memory is a
//!   projection surface, and signal, network, and GPU origins are future
//!   contexts),
//! - the binding layer adapts coordinate paths to origin offsets and is the
//!   CoordSpace persistence backend,
//! - the kv layer implements the tagma-kv CoordKV contract over the
//!   binding backend: the materialized key-value surface over file origins.
//!
//! A protocol is origin-agnostic and an origin is protocol-agnostic. The
//! protocol surface is the tagma-kv CoordKV contract, owned by tagma; chton
//! provides the materialization backends that protocols bind to.

pub mod binding;
pub mod cell;
pub mod io;
pub mod kv;
pub mod origin;
pub mod store;
