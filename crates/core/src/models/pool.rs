//! Tiny session pool for ONNX models that need exclusive `&mut` to run.
//!
//! `ort::Session` is `Send` but not `Sync`, so previously CLIP + YuNet
//! serialized every inference across rayon workers behind a single `Mutex`.
//! For shoots dominated by feature extraction (1000+ photos, RAW decode
//! already parallel), that mutex becomes the bottleneck.
//!
//! This pool holds N independent sessions. Each call tries every session
//! non-blockingly first; only blocks (on a worker-stable slot) when every
//! session is busy. Pool size is configurable via
//! `PHOTO_PICK_INFERENCE_POOL_SIZE` (default 2). Each entry costs one full
//! model copy in RAM — CLIP ≈ 150 MB, YuNet ≈ 1 MB.

use std::sync::Mutex;

pub struct SessionPool<T> {
    sessions: Vec<Mutex<T>>,
}

impl<T> SessionPool<T> {
    pub fn new(items: Vec<T>) -> Self {
        assert!(!items.is_empty(), "session pool needs at least one entry");
        Self {
            sessions: items.into_iter().map(Mutex::new).collect(),
        }
    }

    /// Borrow a session for the duration of `f`. Tries each session
    /// non-blockingly first; only blocks (on a worker-stable slot) when every
    /// session is busy. The closure receives a mutable reference because the
    /// underlying ONNX sessions require `&mut self` for `run`.
    pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        for m in &self.sessions {
            if let Ok(mut g) = m.try_lock() {
                return f(&mut g);
            }
        }
        // All contended — pick a deterministic slot keyed by the rayon worker
        // index so different workers prefer different slots (avoids a thundering
        // herd on slot 0).
        let idx = rayon::current_thread_index().unwrap_or(0) % self.sessions.len();
        let mut g = self.sessions[idx]
            .lock()
            .expect("session pool mutex poisoned");
        f(&mut g)
    }

    pub fn len(&self) -> usize {
        self.sessions.len()
    }
}

/// Read the pool size from `PHOTO_PICK_INFERENCE_POOL_SIZE`. Defaults to 2,
/// which doubles ONNX RAM but typically halves wall-clock for CPU bound
/// extraction. Set to `1` to restore the pre-pool behaviour (single shared
/// session).
pub fn default_size() -> usize {
    std::env::var("PHOTO_PICK_INFERENCE_POOL_SIZE")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(2)
}
