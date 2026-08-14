// ── chton io: CoordKVStore-backed FileIo backend ─────────────────────
//
// Bridges the flat key-space IO surface (FileIo) to the CoordKV store
// (CoordKVStore). Each path maps to a deterministic length-prefixed key:
// axis 0 holds the byte length, axes 1..N-1 hold the path bytes. The
// mapping is injective for paths of 1..=N-1 bytes (no hashing); longer
// paths are rejected. The value carries the path so prefix listing can
// recover it.
//
// The record boundary here is the seam for a future codec layer: the
// value encoding is local to this backend.

use std::sync::Mutex;

use tagma_kv::coord_gen::CoordKey;

use crate::io::{BufferIo, FileIo, IoFuture};
use crate::kv::CoordKVStore;

/// Maximum io depth. The path length prefix is one byte (1..=255), so a
/// depth above 256 would let a path overflow it. Depths above this are
/// rejected at compile time.
pub const MAX_IO_DEPTH: usize = 256;

/// Encode a path and content into a value: `[u32le path_len][path][content]`.
fn encode_value(path: &str, content: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + path.len() + content.len());
    out.extend_from_slice(&(path.len() as u32).to_le_bytes());
    out.extend_from_slice(path.as_bytes());
    out.extend_from_slice(content);
    out
}

/// Decode a value back into `(path, content)`.
fn decode_value(value: &[u8]) -> Result<(String, Vec<u8>), String> {
    if value.len() < 4 {
        return Err("coord-kv value too short".into());
    }
    let path_len = u32::from_le_bytes(value[..4].try_into().unwrap()) as usize;
    let path_bytes = value
        .get(4..4 + path_len)
        .ok_or_else(|| "coord-kv value path truncated".to_string())?;
    let path = std::str::from_utf8(path_bytes)
        .map_err(|e| format!("coord-kv value path not utf-8: {e}"))?
        .to_string();
    Ok((path, value[4 + path_len..].to_vec()))
}

/// The key for a path: length-prefixed bytes. Axis 0 holds the byte
/// length (1..=255), axes 1..N-1 hold the path bytes. Injective for
/// paths of 1..=N-1 bytes; longer paths are rejected. No hashing.
fn key_of<const N: usize>(path: &str) -> Result<CoordKey<N>, String> {
    let bytes = path.as_bytes();
    let len = bytes.len();
    if len == 0 {
        return Err("coord-kv io: empty path".into());
    }
    if len > N - 1 {
        return Err(format!(
            "coord-kv io: path of {len} bytes exceeds the {}-byte capacity",
            N - 1
        ));
    }
    let mut out = [0u8; N];
    // The length prefix is one byte; the depth bound in `new` keeps
    // len <= N-1 <= 255, and try_from guards against truncation.
    out[0] = u8::try_from(len).map_err(|_| {
        format!("coord-kv io: path of {len} bytes exceeds the 255-byte length prefix")
    })?;
    out[1..=len].copy_from_slice(bytes);
    Ok(CoordKey::new(out))
}

/// A FileIo backend over a CoordKV store.
///
/// The store is interior-mutable because `FileIo` methods take `&self`
/// while `CoordKVStore` writes need `&mut self`.
pub struct CoordKVStoreIo<const N: usize> {
    kv: Mutex<CoordKVStore<N>>,
}

impl<const N: usize> CoordKVStoreIo<N> {
    pub fn new(kv: CoordKVStore<N>) -> Self {
        const {
            assert!(
                N >= 1,
                "depth must be at least 1: axis 0 holds the length prefix"
            );
            assert!(
                N <= MAX_IO_DEPTH,
                "depth exceeds the compile-time capacity bound"
            );
        }
        Self { kv: Mutex::new(kv) }
    }
}

impl<const N: usize> BufferIo for CoordKVStoreIo<N> {
    fn is_buffered(&self) -> bool {
        self.kv.lock().unwrap().is_buffered()
    }

    fn flush<'a>(&'a self) -> IoFuture<'a, ()> {
        let kv = &self.kv;
        Box::pin(async move { kv.lock().unwrap().flush().map_err(|e| e.to_string()) })
    }
}

impl<const N: usize> FileIo for CoordKVStoreIo<N> {
    fn read<'a>(&'a self, path: &'a str) -> IoFuture<'a, Option<Vec<u8>>> {
        let kv = &self.kv;
        Box::pin(async move {
            let guard = kv.lock().unwrap();
            let key = key_of::<N>(path)?;
            match guard.get_path(&key.to_coord_path()) {
                Ok(Some(value)) => Ok(Some(decode_value(&value)?.1)),
                Ok(None) => Ok(None),
                Err(e) => Err(e.to_string()),
            }
        })
    }

    fn write<'a>(&'a self, path: &'a str, data: &'a [u8]) -> IoFuture<'a, ()> {
        let kv = &self.kv;
        Box::pin(async move {
            let mut guard = kv.lock().unwrap();
            let key = key_of::<N>(path)?;
            guard
                .put_path(&key.to_coord_path(), &encode_value(path, data))
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
    }

    fn list<'a>(&'a self, prefix: &'a str) -> IoFuture<'a, Vec<String>> {
        let kv = &self.kv;
        Box::pin(async move {
            let guard = kv.lock().unwrap();
            let entries = guard.iter().map_err(|e| e.to_string())?;
            let mut out = Vec::new();
            for (_, value) in entries {
                if let Ok((path, _)) = decode_value(&value)
                    && path.starts_with(prefix)
                {
                    out.push(path);
                }
            }
            Ok(out)
        })
    }

    fn delete<'a>(&'a self, path: &'a str) -> IoFuture<'a, ()> {
        let kv = &self.kv;
        Box::pin(async move {
            let mut guard = kv.lock().unwrap();
            let key = key_of::<N>(path)?;
            guard
                .remove_path(&key.to_coord_path())
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
    }
}
