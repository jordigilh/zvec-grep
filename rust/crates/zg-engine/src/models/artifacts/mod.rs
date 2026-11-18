//! Model artifact download, validation, locking, and publication.

mod cache_lock;
mod downloader;
mod error;
mod http_client;
mod identity;
mod manifest;
mod progress;
mod publish;
mod source;
mod spec;
#[cfg(test)]
mod test_support;

pub(super) use downloader::{ResolveArtifacts, resolve_model_artifacts};
pub(super) use http_client::ArtifactHttpClient;
#[cfg(test)]
pub(super) use progress::ArtifactDownloadProgress;
pub(super) use progress::ModelDownloadProgressReporter;
pub(super) use source::ArtifactSource;
pub(super) use spec::{ArtifactConfig, ArtifactDownloadConfig, ArtifactSourceConfig};
