#[cfg(feature = "onnx")]
use crate::error::{Error, Result};
#[cfg(feature = "onnx")]
use ort::session::{Session, builder::SessionBuilder};
#[cfg(feature = "onnx")]
use std::path::Path;

/// Hardware backend for ONNX inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionProvider {
    Cpu,
    /// CUDA on NVIDIA GPUs (requires `cuda` cargo feature).
    Cuda,
    /// Apple CoreML (requires `coreml` cargo feature).
    CoreMl,
    /// Microsoft DirectML on Windows (requires `directml` cargo feature).
    DirectMl,
}

impl Default for ExecutionProvider {
    fn default() -> Self {
        Self::Cpu
    }
}

/// List the execution providers actually compiled into this build. CPU is
/// always present; GPU variants are gated by cargo features. Used by the
/// server's `/api/providers` capability endpoint so the UI only offers
/// providers that won't silently fall back to CPU.
pub fn available_providers() -> Vec<ExecutionProvider> {
    // `mut` is needed iff any GPU feature is compiled in; in the common
    // CPU-only build the push calls disappear under cfg and the binding
    // would trip unused_mut.
    #[allow(unused_mut)]
    let mut out = vec![ExecutionProvider::Cpu];
    #[cfg(feature = "cuda")]
    out.push(ExecutionProvider::Cuda);
    #[cfg(feature = "coreml")]
    out.push(ExecutionProvider::CoreMl);
    #[cfg(feature = "directml")]
    out.push(ExecutionProvider::DirectMl);
    out
}

/// Build a Session for an ONNX model, configuring the requested provider.
/// Falls back to CPU with a warning when the requested provider isn't
/// compiled in or fails to initialize.
#[cfg(feature = "onnx")]
pub fn build_session(model_path: &Path, ep: ExecutionProvider) -> Result<Session> {
    let mut builder: SessionBuilder = Session::builder()
        .map_err(|e| Error::Config(format!("ort session builder: {e}")))?;

    if let Some(threads) = std::thread::available_parallelism().ok().map(|n| n.get()) {
        builder = builder
            .with_intra_threads(threads.min(8))
            .map_err(|e| Error::Config(format!("ort threads: {e}")))?;
    }

    builder = apply_execution_provider(builder, ep)?;

    builder
        .commit_from_file(model_path)
        .map_err(|e| Error::Config(format!("load model {}: {e}", model_path.display())))
}

#[cfg(all(feature = "onnx", feature = "cuda"))]
fn apply_execution_provider(
    builder: SessionBuilder,
    ep: ExecutionProvider,
) -> Result<SessionBuilder> {
    use ort::execution_providers::CUDAExecutionProvider;
    if ep == ExecutionProvider::Cuda {
        match builder.with_execution_providers([CUDAExecutionProvider::default().build()]) {
            Ok(b) => return Ok(b),
            Err(e) => tracing::warn!("CUDA EP failed ({e}); falling back to CPU"),
        }
    }
    Ok(builder)
}

#[cfg(all(feature = "onnx", feature = "coreml"))]
fn apply_execution_provider(
    builder: SessionBuilder,
    ep: ExecutionProvider,
) -> Result<SessionBuilder> {
    use ort::execution_providers::CoreMLExecutionProvider;
    if ep == ExecutionProvider::CoreMl {
        match builder.with_execution_providers([CoreMLExecutionProvider::default().build()]) {
            Ok(b) => return Ok(b),
            Err(e) => tracing::warn!("CoreML EP failed ({e}); falling back to CPU"),
        }
    }
    Ok(builder)
}

#[cfg(all(feature = "onnx", not(any(feature = "cuda", feature = "coreml", feature = "directml"))))]
fn apply_execution_provider(
    builder: SessionBuilder,
    ep: ExecutionProvider,
) -> Result<SessionBuilder> {
    if ep != ExecutionProvider::Cpu {
        tracing::warn!(
            requested = ?ep,
            "this build has no GPU execution providers compiled in; using CPU"
        );
    }
    Ok(builder)
}
