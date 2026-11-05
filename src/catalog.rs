//! Which models each backend offers, and which effort levels each model accepts.

use crate::openai;
use crate::provider::Result;
use serde_json::Value;

pub const BACKENDS: [&str; 4] = ["claude", "codex", "local", "openrouter"];
pub const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1";

const CLAUDE_MODELS: [&str; 4] = ["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5", "claude-fable-5-1"];
const CLAUDE_EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
const BASIC_EFFORTS: [&str; 3] = ["low", "medium", "high"];

pub struct ModelOption {
    pub id: String,
    /// Empty = the model has no effort setting.
    pub efforts: Vec<String>,
}

impl ModelOption {
    fn new(id: impl Into<String>, efforts: &[&str]) -> Self {
        Self { id: id.into(), efforts: efforts.iter().map(|e| e.to_string()).collect() }
    }
}

pub fn local_base_url() -> String {
    std::env::var("LOCAL_BASE_URL").unwrap_or_else(|_| "http://localhost:1234/v1".into())
}

/// Models offered by `backend`. May hit the network (local, openrouter) or disk (codex).
pub fn models_for(backend: &str) -> Result<Vec<ModelOption>> {
    match backend {
        "claude" => Ok(CLAUDE_MODELS.iter().map(|m| ModelOption::new(*m, &CLAUDE_EFFORTS)).collect()),
        "codex" => codex_models(),
        "local" => Ok(openai::chat_model_ids(&local_base_url())?
            .into_iter()
            .map(|id| ModelOption::new(id, &BASIC_EFFORTS))
            .collect()),
        "openrouter" => openrouter_models(),
        other => Err(format!("unknown backend `{other}`").into()),
    }
}

/// Codex keeps the models your account can use, with their effort levels, in a local cache.
fn codex_models() -> Result<Vec<ModelOption>> {
    let home = std::env::var("HOME")?;
    let path = format!("{home}/.codex/models_cache.json");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {path} (run codex once): {e}"))?;
    let cache: Value = serde_json::from_str(&text)?;
    Ok(cache["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["visibility"] == "list")
        .filter_map(|m| {
            let efforts = m["supported_reasoning_levels"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|e| e["effort"].as_str().map(str::to_owned))
                .collect();
            Some(ModelOption { id: m["slug"].as_str()?.to_owned(), efforts })
        })
        .collect())
}

/// OpenRouter's public model list; effort only for models that support reasoning.
fn openrouter_models() -> Result<Vec<ModelOption>> {
    Ok(openai::list_models(OPENROUTER_URL)?
        .iter()
        .filter_map(|m| {
            let id = m["id"].as_str()?;
            let reasons = m["supported_parameters"].as_array().is_some_and(|p| p.iter().any(|x| x == "reasoning"));
            Some(ModelOption::new(id, if reasons { &BASIC_EFFORTS } else { &[] }))
        })
        .collect())
}
