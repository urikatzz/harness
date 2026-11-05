use crate::provider::{Message, Provider, Result};
use serde_json::{json, Value};

/// Any server speaking the OpenAI `/chat/completions` format:
/// OpenRouter, LM Studio, Ollama, llama.cpp, vLLM, OpenAI itself.
pub struct OpenAiCompatible {
    base_url: String,
    api_key: Option<String>,
    model: String,
}

impl OpenAiCompatible {
    pub fn new(base_url: impl Into<String>, api_key: Option<String>, model: impl Into<String>) -> Self {
        Self { base_url: base_url.into(), api_key, model: model.into() }
    }
}

/// First non-embedding model a server advertises at `/models`.
pub fn first_chat_model(base_url: &str) -> Result<String> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let body: Value = ureq::get(&url).call()?.into_json()?;
    body["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["id"].as_str())
        .find(|id| !id.contains("embed"))
        .map(str::to_owned)
        .ok_or_else(|| format!("no chat models listed at {url}").into())
}

impl Provider for OpenAiCompatible {
    fn complete(&self, history: &[Message]) -> Result<String> {
        let messages: Vec<Value> = history
            .iter()
            .map(|m| json!({ "role": m.role.as_str(), "content": m.content }))
            .collect();
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        let mut req = ureq::post(&url);
        if let Some(key) = &self.api_key {
            req = req.set("Authorization", &format!("Bearer {key}"));
        }

        let body: Value = match req.send_json(json!({ "model": self.model, "messages": messages })) {
            Ok(resp) => resp.into_json()?,
            // Surface the server's error body; it usually says what's wrong (bad model, bad key).
            Err(ureq::Error::Status(code, resp)) => {
                return Err(format!("HTTP {code}: {}", resp.into_string().unwrap_or_default()).into())
            }
            Err(e) => return Err(e.into()),
        };

        body["choices"][0]["message"]["content"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("unexpected response: {body}").into())
    }
}
