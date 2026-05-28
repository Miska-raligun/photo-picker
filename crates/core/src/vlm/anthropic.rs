use super::{VlmProvider, VlmRequest};
use crate::error::{Error, Result};
use base64::Engine;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

const DEFAULT_MODEL: &str = "claude-opus-4-7";
const DEFAULT_ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";

pub struct AnthropicProvider {
    api_key: String,
    model: String,
    base_url: String,
}

impl AnthropicProvider {
    /// Construct from environment.
    ///
    /// - `ANTHROPIC_API_KEY` (required) — x-api-key value.
    /// - `ANTHROPIC_MODEL` (optional) — defaults to `claude-opus-4-7`.
    /// - `ANTHROPIC_BASE_URL` (optional) — full messages endpoint.
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| Error::Config("ANTHROPIC_API_KEY not set".into()))?;
        let model = std::env::var("ANTHROPIC_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into());
        let base_url = std::env::var("ANTHROPIC_BASE_URL")
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

impl VlmProvider for AnthropicProvider {
    fn name(&self) -> &str { "anthropic" }
    fn model(&self) -> &str { &self.model }

    fn complete(&self, req: &VlmRequest) -> Result<String> {
        // Label-then-image interleave; main prompt last so it acts as the
        // instruction over all attached evidence.
        let mut content_parts: Vec<serde_json::Value> = Vec::with_capacity(req.images.len() * 2 + 1);
        for (i, img) in req.images.iter().enumerate() {
            content_parts.push(json!({
                "type": "text",
                "text": format!("Image {} ({}):", i + 1, img.label)
            }));
            let b64 = base64::engine::general_purpose::STANDARD.encode(&img.jpeg_bytes);
            content_parts.push(json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": "image/jpeg",
                    "data": b64,
                }
            }));
        }
        content_parts.push(json!({ "type": "text", "text": req.user_prompt }));

        let mut body = json!({
            "model": self.model,
            "max_tokens": req.max_tokens,
            "messages": [{ "role": "user", "content": content_parts }],
        });
        if let Some(sys) = &req.system {
            body["system"] = json!(sys);
        }

        tracing::info!(
            base_url = %self.base_url,
            model = %self.model,
            n_images = req.images.len(),
            "vlm anthropic request start"
        );
        let start = std::time::Instant::now();
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(180)))
            .proxy(ureq::Proxy::try_from_env())
            // See OpenAI provider: keep 4xx/5xx responses so the body's error
            // message reaches the user instead of a bare status code.
            .http_status_as_error(false)
            .build()
            .into();

        let mut resp = agent
            .post(&self.base_url)
            .header("x-api-key", self.api_key.as_str())
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .send_json(&body)
            .map_err(|e| {
                tracing::warn!(elapsed_ms = start.elapsed().as_millis() as u64, %e, "vlm anthropic request failed");
                Error::Config(format!("anthropic request: {e}"))
            })?;
        let status = resp.status();
        tracing::info!(
            elapsed_ms = start.elapsed().as_millis() as u64,
            status = status.as_u16(),
            "vlm anthropic response"
        );
        if !status.is_success() {
            let body = resp.body_mut().read_to_string().unwrap_or_default();
            let snippet: String = body.trim().chars().take(500).collect();
            tracing::warn!(status = status.as_u16(), body = %snippet, "vlm anthropic error response");
            return Err(Error::Config(format!(
                "anthropic HTTP {}: {}",
                status.as_u16(),
                if snippet.is_empty() { "(empty body)" } else { &snippet }
            )));
        }

        let parsed: MessagesResponse = resp
            .body_mut()
            .read_json()
            .map_err(|e| Error::Config(format!("anthropic response parse: {e}")))?;

        parsed
            .content
            .into_iter()
            .find(|b| b.kind == "text")
            .map(|b| b.text)
            .ok_or_else(|| Error::Config("anthropic: no text block in response".into()))
    }
}

#[derive(Debug, Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlock>,
}

#[derive(Debug, Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: String,
}
