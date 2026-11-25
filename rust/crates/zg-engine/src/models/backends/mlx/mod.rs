//! Apple-GPU embedding backend (issue #74), opt-in behind the `mlx` feature.
//!
//! Scaffold stage: catalog identity, backend identity, and the construction
//! path the runtime and factory use. The worker thread (`mlx_rs::Array` is
//! `!Send`), weight loading, and the `JinaBERT` forward pass land in the
//! following plan steps (benchmarks/mlx-gpu/README.md); until then `embed`
//! reports a structured unsupported error rather than pretending to work.

use async_trait::async_trait;

use crate::{
    domain::Content,
    domain::model::{EmbeddingModelInfo, EmbeddingResult, ModelConfig, ModelInfo},
    models::{
        catalog::TransformersConfig,
        spi::{EmbeddingModel, EmbeddingOptions, ModelError},
    },
};

pub(crate) struct MlxEmbeddingModel {
    info: EmbeddingModelInfo,
}

impl MlxEmbeddingModel {
    pub(crate) fn new(entry: TransformersConfig, _options: ModelConfig) -> Self {
        Self {
            info: EmbeddingModelInfo {
                model: ModelInfo {
                    provider: entry.provider.to_owned(),
                    name: entry.model.to_owned(),
                    endpoint: None,
                },
                dimension: entry.dimension,
                metric: entry.metric,
                max_batch_size: entry.max_batch_size,
                max_input_tokens: Some(entry.max_input_tokens),
                max_image_bytes: None,
                backend: Some("mlx-gpu".to_owned()),
                dtype: Some(entry.dtype.to_owned()),
            },
        }
    }
}

#[async_trait]
impl EmbeddingModel for MlxEmbeddingModel {
    fn info(&self) -> &EmbeddingModelInfo {
        &self.info
    }

    async fn embed(
        &self,
        _inputs: &[Vec<Content>],
        _options: EmbeddingOptions,
    ) -> Result<EmbeddingResult, ModelError> {
        Err(ModelError::unsupported(
            "MLX embedding execution is not implemented yet",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::MlxEmbeddingModel;
    use crate::domain::model::ModelConfig;
    use crate::models::catalog::get_embedding_model_catalog_entry;
    use crate::models::spi::EmbeddingModel;

    #[test]
    fn catalog_entry_reports_its_own_backend_identity() {
        let entry = get_embedding_model_catalog_entry("local/jina-embeddings-v2-base-code-mlx")
            .expect("MLX catalog entry is compiled in with --features mlx");
        let crate::models::catalog::EmbeddingCatalogEntry::Mlx(config) = entry else {
            panic!("MLX reference must resolve to the MLX entry");
        };
        let model = MlxEmbeddingModel::new(config, ModelConfig::default());
        let info = model.info();
        info.validate().expect("valid model info");
        assert_eq!(info.backend.as_deref(), Some("mlx-gpu"));
        assert_eq!(info.dtype.as_deref(), Some("fp16"));
        assert_eq!(info.dimension, 768);
        assert_eq!(info.max_batch_size, 8);
        assert_eq!(
            info.model.reference(),
            "local/jina-embeddings-v2-base-code-mlx"
        );
    }
}
