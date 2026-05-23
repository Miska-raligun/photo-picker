use super::{VlmProvider, VlmRequest};
use crate::error::{Error, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

const DEFAULT_MODEL: &str = "gpt-4o";
const ENDPOINT: &str = "https://api.openai.com/v1/chat/completions";

pub struct OpenAiProvider {
    api_key: String,
    model: String,
    base_url: String,
}

impl OpenAiProvider {
    /// Construct from the `OPENAI_API_KEY` environment variable. Returns
    /// `Error::Config` if the variable isn't set.
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .map_err(|_| Error::Config("OPENAI_API_KEY not set".into()))?;
        let model = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into());
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

impl VlmProvider for OpenAiProvider {
    fn name(&self) -> &str { "openai" }
    fn model(&self) -> &str { &self.model }

    fn complete(&self, req: &VlmRequest) -> Result<String> {
        let mut content_parts: Vec<serde_json::Value> = Vec::with_capacity(req.images.len() + 1);
        content_parts.push(json!({ "type": "text", "text": req.user_prompt }));
        for img in &req.images {
            let b64 = base64::engine::general_purpose::STANDARD.encode(&img.jpeg_bytes);
            content_parts.push(json!({
                "type": "image_url",
                "image_url": { "url": format!("data:image/jpeg;base64,{b64}") }
            }));
        }

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

        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(120)))
            .build()
            .into();

        let mut resp = agent
            .post(&self.base_url)
            .header("authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
            .send_json(&body)
            .map_err(|e| Error::Config(format!("openai request: {e}")))?;

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
