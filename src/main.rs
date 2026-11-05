mod catalog;
mod cli;
mod openai;
mod picker;
mod provider;
mod repl;

use catalog::{local_base_url, OPENROUTER_URL};
use cli::CliProvider;
use openai::{EffortStyle, OpenAiCompatible};
use provider::{Message, Provider, Result, Role, Selection};
use repl::Repl;

const DEFAULT_BACKEND: &str = "claude";

const USAGE: &str = "usage: uritheking [openrouter|local|claude|codex] [model] [--effort level] [-p prompt]

  no arguments  opens the uriTheKing chat with the claude backend (type /help inside)

  openrouter  needs OPENROUTER_API_KEY            (default model: openrouter/auto)
  local       OpenAI-compatible server, LOCAL_BASE_URL (default http://localhost:1234/v1)
              (default model: first one the server lists; Ollama: http://localhost:11434/v1)
  claude      runs `claude -p`, uses your Claude Code login
  codex       runs `codex exec`, uses your Codex login

  --effort    reasoning effort, e.g. low | medium | high (levels depend on the model)
  -p prompt   ask once and exit; otherwise opens the chat";

fn build_provider(sel: &Selection) -> Result<Box<dyn Provider>> {
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
        other => return Err(format!("unknown provider `{other}`\n\n{USAGE}").into()),
    })
}

fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut positional = Vec::new();
    let mut prompt = None;
    let mut effort = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "-p" => prompt = Some(args.next().ok_or("-p needs a prompt")?),
            "--effort" => effort = Some(args.next().ok_or("--effort needs a level")?),
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            _ => positional.push(a),
        }
    }
    let mut positional = positional.into_iter();
    let selection = Selection {
        backend: positional.next().unwrap_or_else(|| DEFAULT_BACKEND.into()),
        model: positional.next(),
        effort,
    };

    match prompt {
        Some(p) => {
            let reply = build_provider(&selection)?.complete(&[Message { role: Role::User, content: p }])?;
            println!("{reply}");
            Ok(())
        }
        None => Repl::new(build_provider, selection)?.run(),
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
