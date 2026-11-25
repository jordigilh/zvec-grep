use super::{Metric, ModelInfo};
use crate::{EngineError, EngineResult};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EmbeddingModelInfo {
    pub model: ModelInfo,
    pub dimension: usize,
    pub metric: Metric,
    /// Maximum number of inputs accepted by one embedding request.
    pub max_batch_size: usize,
    /// Positive token limit when applicable and known.
    pub max_input_tokens: Option<usize>,
    /// Positive image size limit in bytes when applicable and known.
    pub max_image_bytes: Option<usize>,
    /// Embedding runtime that produced the vectors, e.g. `onnx-cpu`.
    /// Recorded by the backend from its actual execution path, never user config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    /// Numeric format of the embedding artifact, e.g. `q8` or `fp32`.
    /// Recorded together with `backend` so both are present or both are absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dtype: Option<String>,
}

/// Identity of the embedding runtime behind an index generation.
///
/// Embeddings drift across runtimes (ONNX builds, quantization dtypes) by the
/// same magnitude as model-choice gaps, so an index may only be served by the
/// backend identity that produced it. `None` means the backend does not record
/// an identity (remote APIs) or the index predates backend recording.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BackendIdentity {
    pub backend: String,
    pub dtype: String,
}

impl fmt::Display for BackendIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "backend={} dtype={}", self.backend, self.dtype)
    }
}

impl EmbeddingModelInfo {
    pub(crate) fn validate(&self) -> EngineResult<()> {
        for (field, value) in [
            ("provider", self.model.provider.as_str()),
            ("name", self.model.name.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(EngineError::invalid_argument(format!(
                    "embedding model {field} must be non-empty",
                )));
            }
        }
        for (field, value) in [
            ("dimension", Some(self.dimension)),
            ("max_batch_size", Some(self.max_batch_size)),
            ("max_input_tokens", self.max_input_tokens),
            ("max_image_bytes", self.max_image_bytes),
        ] {
            if value == Some(0) {
                return Err(EngineError::invalid_argument(format!(
                    "embedding model {field} must be greater than zero",
                )));
            }
        }
        match (&self.backend, &self.dtype) {
            (None, None) => {}
            (Some(backend), Some(dtype)) => {
                for (field, value) in [("backend", backend), ("dtype", dtype)] {
                    if value.trim().is_empty() {
                        return Err(EngineError::invalid_argument(format!(
                            "embedding model {field} must be non-empty",
                        )));
                    }
                }
            }
            _ => {
                return Err(EngineError::invalid_argument(
                    "embedding model backend and dtype must be recorded together",
                ));
            }
        }
        Ok(())
    }

    /// The runtime identity recorded for this index, when both parts are present.
    pub(crate) fn backend_identity(&self) -> Option<BackendIdentity> {
        let identity = BackendIdentity {
            backend: self.backend.clone()?,
            dtype: self.dtype.clone()?,
        };
        Some(identity)
    }

    /// Check the fields that determine whether an existing index can be reused.
    pub(crate) fn ensure_index_compatible(&self, other: &Self) -> EngineResult<()> {
        if self.model.provider != other.model.provider
            || self.model.name != other.model.name
            || self.dimension != other.dimension
            || self.metric != other.metric
            || self.max_input_tokens != other.max_input_tokens
            || self.max_image_bytes != other.max_image_bytes
        {
            return Err(EngineError::invalid_argument(
                "existing index uses a different embedding model; rebuild the index",
            ));
        }
        if (self.backend.as_deref(), self.dtype.as_deref())
            != (other.backend.as_deref(), other.dtype.as_deref())
        {
            return Err(backend_identity_mismatch(
                self.backend_identity().as_ref(),
                other.backend_identity().as_ref(),
            ));
        }
        Ok(())
    }
}

/// Structured refusal when the serving runtime differs from the stored one.
///
/// Indexes built before backend recording have no identity; every such index
/// was produced by a runtime older than the current binary, so the unknown
/// identity also refuses and forces a one-time rebuild.
fn backend_identity_mismatch(
    stored: Option<&BackendIdentity>,
    current: Option<&BackendIdentity>,
) -> EngineError {
    let stored = stored.map_or_else(
        || "no recorded identity (built before backend recording)".to_owned(),
        ToString::to_string,
    );
    let current = current.map_or_else(|| "no recorded identity".to_owned(), ToString::to_string);
    EngineError::index_backend_mismatch(format!(
        "workspace index embedding backend identity does not match this runtime: \
         index has {stored}, this runtime has {current}; \
         rebuild the index with `zg --index --rebuild`"
    ))
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EmbeddingPurpose {
    #[default]
    Document,
    Query,
}

/// Result of a batch embedding request.
#[derive(Clone, Debug, PartialEq)]
pub struct EmbeddingResult {
    /// One vector per input, in input order, each with the model's declared dimension.
    pub vectors: Vec<Vec<f32>>,
    /// Zero-based indices of inputs that were truncated before producing their vectors.
    pub truncated: Vec<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> EmbeddingModelInfo {
        EmbeddingModelInfo {
            model: ModelInfo {
                provider: "local".into(),
                name: "fixture".into(),
                endpoint: None,
            },
            dimension: 8,
            metric: Metric::Cosine,
            max_batch_size: 4,
            max_input_tokens: Some(128),
            max_image_bytes: None,
            backend: None,
            dtype: None,
        }
    }

    fn with_identity(
        mut info: EmbeddingModelInfo,
        backend: &str,
        dtype: &str,
    ) -> EmbeddingModelInfo {
        info.backend = Some(backend.to_owned());
        info.dtype = Some(dtype.to_owned());
        info
    }

    #[test]
    fn backend_and_dtype_round_trip_and_default_to_absent() {
        let recorded = with_identity(fixture(), "onnx-cpu", "q8");
        assert_eq!(
            recorded.backend_identity().as_ref(),
            Some(&BackendIdentity {
                backend: "onnx-cpu".to_owned(),
                dtype: "q8".to_owned()
            })
        );
        let value = serde_json::to_value(&recorded).expect("serialize");
        assert_eq!(value["backend"], "onnx-cpu");
        assert_eq!(value["dtype"], "q8");
        assert_eq!(
            serde_json::from_value::<EmbeddingModelInfo>(value).expect("deserialize"),
            recorded
        );
        // A legacy record has neither field and reports no identity.
        let mut legacy = serde_json::to_value(fixture()).expect("serialize");
        let object = legacy.as_object_mut().expect("object");
        object.remove("backend");
        object.remove("dtype");
        let decoded = serde_json::from_value::<EmbeddingModelInfo>(legacy).expect("legacy decode");
        assert_eq!(decoded.backend, None);
        assert_eq!(decoded.dtype, None);
        assert_eq!(decoded.backend_identity(), None);
        assert_eq!(decoded, fixture());
    }

    #[test]
    fn backend_and_dtype_must_be_recorded_together_and_non_empty() {
        let mut partial = fixture();
        partial.backend = Some("onnx-cpu".into());
        assert!(partial.validate().is_err());
        partial.dtype = Some(" ".into());
        assert!(partial.validate().is_err());
        partial.dtype = Some("q8".into());
        assert!(partial.validate().is_ok());
    }

    #[test]
    fn same_backend_identity_stays_compatible() {
        let stored = with_identity(fixture(), "onnx-cpu", "q8");
        let current = with_identity(fixture(), "onnx-cpu", "q8");
        stored
            .ensure_index_compatible(&current)
            .expect("compatible");
        // Backends without an identity (remote APIs) match each other.
        fixture()
            .ensure_index_compatible(&fixture())
            .expect("compatible");
    }

    #[test]
    fn a_different_backend_or_dtype_refuses_with_both_identities_named() {
        for (stored_backend, stored_dtype, current_backend, current_dtype) in [
            ("onnx-cpu", "q8", "onnx-coreml", "q8"),
            ("onnx-cpu", "q8", "onnx-cpu", "fp32"),
            ("model2vec", "safetensors", "onnx-cpu", "q8"),
        ] {
            let stored = with_identity(fixture(), stored_backend, stored_dtype);
            let current = with_identity(fixture(), current_backend, current_dtype);
            let error = stored
                .ensure_index_compatible(&current)
                .expect_err("cross-backend queries must refuse");
            assert_eq!(
                error.code(),
                EngineError::INDEX_BACKEND_MISMATCH,
                "{stored_backend}"
            );
            assert!(!error.is_retryable());
            assert!(
                error.message().contains(stored_backend),
                "{}",
                error.message()
            );
            assert!(
                error.message().contains(stored_dtype),
                "{}",
                error.message()
            );
            assert!(
                error.message().contains(current_backend),
                "{}",
                error.message()
            );
            assert!(
                error.message().contains(current_dtype),
                "{}",
                error.message()
            );
            assert!(error.message().contains("rebuild"), "{}", error.message());
        }
    }

    #[test]
    fn an_unrecorded_identity_refuses_until_the_index_is_rebuilt() {
        // Legacy indexes predate backend recording; every one was built by a
        // different runtime than this binary, so serving them must fail closed.
        let legacy = fixture();
        let current = with_identity(fixture(), "onnx-cpu", "q8");
        let error = legacy
            .ensure_index_compatible(&current)
            .expect_err("legacy identity must refuse");
        assert_eq!(error.code(), EngineError::INDEX_BACKEND_MISMATCH);
        assert!(
            error.message().contains("built before backend recording"),
            "{}",
            error.message()
        );
        // The reverse direction is equally refused.
        assert_eq!(
            current
                .ensure_index_compatible(&legacy)
                .expect_err("current identity must refuse to serve an unknown index")
                .code(),
            EngineError::INDEX_BACKEND_MISMATCH
        );
    }
}
