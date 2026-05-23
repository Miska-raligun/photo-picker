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
pub mod clip;
pub mod registry;

pub use cache::{ensure_model, ModelDescriptor};
pub use clip::{ClipEncoder, CLIP_EMBED_DIM};
pub use registry::ExecutionProvider;
