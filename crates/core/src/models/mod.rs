//! Model registry, cache, and ORT session helpers (M3+).
//!
//! All ONNX models live under the user's XDG cache directory at
//! `<cache>/photo-pick/models/`. On first use of a model the descriptor is
//! consulted; if the file is missing or its SHA-256 doesn't match, it is
//! downloaded from `url` (typically a HuggingFace `resolve/main/...` link).
//!
//! ## Execution providers
//!
//! CPU is always available. GPU providers (CUDA, CoreML, DirectML) are exposed
//! behind cargo features; missing features fall back to CPU silently with a
//! warning log.

pub mod cache;
#[cfg(feature = "onnx")]
pub mod clip;
#[cfg(feature = "onnx")]
pub mod pool;
pub mod registry;

pub use cache::{ensure_model, ModelDescriptor};
#[cfg(feature = "onnx")]
pub use clip::{ClipEncoder, CLIP_EMBED_DIM};
#[cfg(feature = "onnx")]
pub use pool::{default_size as default_pool_size, SessionPool};
pub use registry::ExecutionProvider;
