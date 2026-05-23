use super::{VlmProvider, VlmRequest};
use crate::error::{Error, Result};
use base64::Engine;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

const DEFAULT_MODEL: &str = "claude-opus-4-7";
const ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";

pub struct AnthropicProvider {
    api_key: String,
    model: String,
    base_url: String,
}

impl AnthropicProvider {
    /// Construct from `ANTHROPIC_API_KEY`. `ANTHROPIC_MODEL` overrides the
    /// default model.
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| Error::Config("ANTHROPIC_API_KEY not set".into()))?;
        let model = std::env::var("ANTHROPIC_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into());
        Ok(Self {
            api_key,
            model,
            base_url: ENDPOINT.into(),
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
}

impl VlmProvider for AnthropicProvider {
    fn name(&self) -> &str { "anthropic" }
    fn model(&self) -> &str { &self.model }

    fn complete(&self, req: &VlmRequest) -> Result<String> {
        let mut content_parts: Vec<serde_json::Value> = Vec::with_capacity(req.images.len() + 1);
        for img in &req.images {
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

        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(120)))
            .build()
            .into();

        let mut resp = agent
            .post(&self.base_url)
            .header("x-api-key", self.api_key.as_str())
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .send_json(&body)
            .map_err(|e| Error::Config(format!("anthropic request: {e}")))?;

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
