//! Backend implementations and the one place that picks between them.

pub mod cli;
pub mod openai;

use crate::catalog::{local_base_url, OPENROUTER_URL};
use crate::provider::{Provider, Result, Selection};
use cli::CliProvider;
use openai::{EffortStyle, OpenAiCompatible};

/// Adding a backend = one struct implementing `Provider` + one arm here (and in `catalog`).
pub fn build_provider(sel: &Selection) -> Result<Box<dyn Provider>> {
    let (model, effort) = (sel.model.clone(), sel.effort.clone());
    Ok(match sel.backend.as_str() {
        "openrouter" => {
            let key = std::env::var("OPENROUTER_API_KEY").map_err(|_| "OPENROUTER_API_KEY is not set")?;
            let model = model.unwrap_or_else(|| "openrouter/auto".into());
            Box::new(OpenAiCompatible::new(OPENROUTER_URL, Some(key), model, effort, EffortStyle::OpenRouter))
        }
        "local" => {
            let base = local_base_url();
            let model = match model {
                Some(m) => m,
                None => openai::first_chat_model(&base)?,
            };
            Box::new(OpenAiCompatible::new(base, None, model, effort, EffortStyle::OpenAi))
        }
        "claude" => Box::new(CliProvider::claude(model, effort)),
        "codex" => Box::new(CliProvider::codex(model, effort)),
        other => return Err(format!("unknown backend `{other}` (use claude, codex, local or openrouter)").into()),
    })
}
