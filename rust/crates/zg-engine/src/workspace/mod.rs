//! Workspace index location, manifest persistence, and process coordination.

pub(crate) mod build;
pub(crate) mod layout;
pub(crate) mod lock;
pub(crate) mod manifest;
pub(crate) mod registry;

/// One format version covers the manifest, storage schema, records and indexing semantics.
/// Version 1 belongs to the released Node.js implementation, and version 2 to the
/// FP32 embedding format. Version 3 stores embedding vectors as IEEE binary16 (FP16).
/// Incompatible indexes require an explicit rebuild.
pub(crate) const CURRENT_INDEX_VERSION: u32 = 3;
