//! Vision-language model providers (M4.3).
//!
//! On demand — never by default — the pipeline can ship a composition group's
//! photos to a remote VLM and ask for a natural-language explanation of why
//! the kept picks beat the rejected ones. Two providers are bundled: OpenAI
//! (gpt-4o / gpt-4.1) and Anthropic (claude-opus / sonnet).
//!
//! All requests are synchronous (ureq under the hood). Callers in async
//! contexts (the axum server) should wrap calls in `tokio::task::spawn_blocking`.
//! API keys come from environment variables — never from disk.
//!
//! Cost discipline: per design decision #4, VLM calls happen only when a user
//! explicitly asks for an explanation (e.g. UI button), and results are
//! cacheable upstream so re-asking the same group costs nothing.

pub mod anthropic;
pub mod openai;

pub use anthropic::AnthropicProvider;
pub use openai::OpenAiProvider;

use crate::error::Result;

/// A single image to include in a multimodal prompt.
#[derive(Debug, Clone)]
pub struct VlmImage {
    /// Pre-encoded JPEG bytes. The pipeline normally feeds in the 256px
    /// thumbnail produced for the HTML report.
    pub jpeg_bytes: Vec<u8>,
    /// Short label the VLM can refer to ("image 1", filename, etc.).
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct VlmRequest {
    pub system: Option<String>,
    pub user_prompt: String,
    pub images: Vec<VlmImage>,
    pub max_tokens: u32,
}

impl VlmRequest {
    pub fn new(user_prompt: impl Into<String>, images: Vec<VlmImage>) -> Self {
        Self {
            system: None,
            user_prompt: user_prompt.into(),
            images,
            max_tokens: 800,
        }
    }
    pub fn with_system(mut self, s: impl Into<String>) -> Self {
        self.system = Some(s.into());
        self
    }
    pub fn with_max_tokens(mut self, n: u32) -> Self {
        self.max_tokens = n;
        self
    }
}

pub trait VlmProvider: Send + Sync {
    /// Provider name for telemetry / logs ("openai", "anthropic").
    fn name(&self) -> &str;
    /// Model identifier in use ("gpt-4o", "claude-opus-4-7", etc.).
    fn model(&self) -> &str;
    /// Synchronously send the request and return the assistant's text reply.
    fn complete(&self, req: &VlmRequest) -> Result<String>;
}

/// Build the explain-group prompt used by the Web UI.
///
/// `lang` is the user's UI language code ("en", "zh"). The model is told
/// nothing about the algorithm's pick so its ranking is independent — the
/// user compares the model's ordering against the algorithm's verdict to
/// spot disagreements.
pub fn explain_group_prompt(scene: &str, _kept_count: usize, total: usize, lang: &str) -> String {
    let response_lang = match lang {
        "zh" => "Simplified Chinese (中文)",
        _ => "English",
    };
    format!(
        "I am sending you EXACTLY {total} photos from the same scene. They are labelled \
         Image 1 through Image {total} in attachment order. Scene type hint: {scene}.\n\n\
         TASK: Rank these {total} photos from best to worst as photographs. Judge by what you \
         actually observe in each image:\n\
         - Focus / sharpness (especially on the main subject)\n\
         - Subject expression and pose (eyes open, natural expression for portraits)\n\
         - Composition and framing\n\
         - Exposure (no clipped highlights or crushed shadows)\n\
         - Motion blur from subject or camera shake\n\
         - Any distracting elements (extras in frame, phones, lens flare)\n\n\
         RULES:\n\
         - Output EXACTLY {total} ranked lines, best first.\n\
         - Format each line as: `Rank N (Image X): <one concrete sentence about that specific photo>`\n\
         - X is the image number from the attachment order. N is your rank.\n\
         - Reference observations you can actually see; do not invent extra images.\n\
         - Do NOT assume any prior labels — judge purely on visual evidence.\n\
         - Total under 300 words.\n\
         - Respond in {response_lang}."
    )
}
