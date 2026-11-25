//! Apple-GPU embedding backend (issue #74), opt-in behind the `mlx` feature.
//!
//! Declarative module: [`model`] owns the catalog-backed SPI implementation,
//! [`worker`] the array-owning thread and its cancellation/panic contract,
//! [`forward`] the `JinaBERT` numerics, and [`weights`] the safetensors layout.
//! `Array` is `!Send`, so nothing in `forward`/`weights` crosses a thread.

mod forward;
mod model;
mod weights;
mod worker;

pub(crate) use model::MlxEmbeddingModel;

#[cfg(test)]
mod tests;
