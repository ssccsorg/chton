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
//! - the protocol layer defines materialization protocols, with key-value as
//!   the first protocol, bound to the backend by the tagma-kv CoordKV
//!   contract.
//!
//! A protocol is origin-agnostic and an origin is protocol-agnostic. Storage
//! is what a disk origin does, transmission is what a signal origin does, and
//! a protocol materializes over either.

pub mod binding;
pub mod io;
pub mod origin;
pub mod protocol;
