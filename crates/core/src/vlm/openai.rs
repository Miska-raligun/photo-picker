use super::{VlmProvider, VlmRequest};
use crate::error::{Error, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

const DEFAULT_MODEL: &str = "gpt-4o";
const DEFAULT_ENDPOINT: &str = "https://api.openai.com/v1/chat/completions";

pub struct OpenAiProvider {
    api_key: String,
    model: String,
    base_url: String,
}

impl OpenAiProvider {
    /// Construct from environment.
    ///
    /// - `OPENAI_API_KEY` (required) — bearer token.
    /// - `OPENAI_MODEL` (optional) — model id, defaults to `gpt-4o`.
    /// - `OPENAI_BASE_URL` (optional) — full chat-completions endpoint.
    ///   Lets you point at any OpenAI-compatible service (SiliconFlow,
    ///   DeepSeek, Together, OpenRouter, etc.). Defaults to OpenAI's URL.
    ///
    /// Example for SiliconFlow + Qwen3-VL:
    /// ```text
    /// OPENAI_API_KEY=sk-...
    /// OPENAI_BASE_URL=https://api.siliconflow.cn/v1/chat/completions
    /// OPENAI_MODEL=Qwen/Qwen3-VL-32B-Instruct
    /// ```
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .map_err(|_| Error::Config("OPENAI_API_KEY not set".into()))?;
        let model = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into());
        let base_url = std::env::var("OPENAI_BASE_URL")
            .unwrap_or_else(|_| DEFAULT_ENDPOINT.into());
        Ok(Self {
            api_key,
            model,
            base_url,
        })
    }

    pub fn with_model(mut self, m: impl Into<String>) -> Self {
        self.model = m.into();
        self
    }

    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Direct constructor for caller-supplied credentials (e.g. UI-stored
    /// per-user API key for an OpenAI-compatible endpoint).
    pub fn from_parts(
        api_key: impl Into<String>,
        model: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Self {
        Self {
            api_key: api_key.into(),
            model: model.into(),
            base_url: base_url.into(),
        }
    }
}

impl VlmProvider for OpenAiProvider {
    fn name(&self) -> &str { "openai" }
    fn model(&self) -> &str { &self.model }

    fn complete(&self, req: &VlmRequest) -> Result<String> {
        // Interleave a labelled text marker before each image so the model can
        // reference them by number, then put the main task prompt LAST so the
        // model treats it as the instruction over the attached evidence.
        let mut content_parts: Vec<serde_json::Value> = Vec::with_capacity(req.images.len() * 2 + 1);
        for (i, img) in req.images.iter().enumerate() {
            content_parts.push(json!({
                "type": "text",
                "text": format!("Image {} ({}):", i + 1, img.label)
            }));
            let b64 = base64::engine::general_purpose::STANDARD.encode(&img.jpeg_bytes);
            content_parts.push(json!({
                "type": "image_url",
                "image_url": { "url": format!("data:image/jpeg;base64,{b64}") }
            }));
        }
        content_parts.push(json!({ "type": "text", "text": req.user_prompt }));

        let mut messages: Vec<serde_json::Value> = Vec::new();
        if let Some(sys) = &req.system {
            messages.push(json!({ "role": "system", "content": sys }));
        }
        messages.push(json!({ "role": "user", "content": content_parts }));

        let body = json!({
            "model": self.model,
            "messages": messages,
            "max_tokens": req.max_tokens,
        });

        tracing::info!(
            base_url = %self.base_url,
            model = %self.model,
            n_images = req.images.len(),
            "vlm openai request start"
        );
        let start = std::time::Instant::now();
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(180)))
            // Honor HTTPS_PROXY / NO_PROXY env vars so foreign endpoints work
            // behind a Clash/v2ray-style proxy without going direct and hanging.
            .proxy(ureq::Proxy::try_from_env())
            // Keep the response on 4xx/5xx instead of collapsing to a bare
            // StatusCode error — the body carries the real reason (invalid key,
            // unknown model, rate limit) we want to show the user.
            .http_status_as_error(false)
            .build()
            .into();

        let mut resp = agent
            .post(&self.base_url)
            .header("authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
            .send_json(&body)
            .map_err(|e| {
                tracing::warn!(elapsed_ms = start.elapsed().as_millis() as u64, %e, "vlm openai request failed");
                Error::Config(format!("openai request: {e}"))
            })?;
        let status = resp.status();
        tracing::info!(
            elapsed_ms = start.elapsed().as_millis() as u64,
            status = status.as_u16(),
            "vlm openai response"
        );
        if !status.is_success() {
            let body = resp.body_mut().read_to_string().unwrap_or_default();
            let snippet: String = body.trim().chars().take(500).collect();
            tracing::warn!(status = status.as_u16(), body = %snippet, "vlm openai error response");
            return Err(Error::Config(format!(
                "openai HTTP {}: {}",
                status.as_u16(),
                if snippet.is_empty() { "(empty body)" } else { &snippet }
            )));
        }

        let parsed: ChatResponse = resp
            .body_mut()
            .read_json()
            .map_err(|e| Error::Config(format!("openai response parse: {e}")))?;

        parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .ok_or_else(|| Error::Config("openai: empty choices".into()))
    }
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}

#[derive(Debug, Deserialize, Serialize)]
struct ChatMessage {
    #[allow(dead_code)]
    #[serde(default)]
    role: String,
    content: String,
}
