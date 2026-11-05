mod cli;
mod openai;
mod provider;

use cli::CliProvider;
use openai::OpenAiCompatible;
use provider::{Message, Provider, Result, Role};
use std::io::{self, BufRead, Write};

const USAGE: &str = "usage: harness <openrouter|local|claude|codex> [model] [-p prompt]

  openrouter  needs OPENROUTER_API_KEY            (default model: openrouter/auto)
  local       OpenAI-compatible server, LOCAL_BASE_URL (default http://localhost:1234/v1)
              (default model: first one the server lists; Ollama: http://localhost:11434/v1)
  claude      runs `claude -p`, uses your Claude Code login
  codex       runs `codex exec`, uses your Codex login

  -p prompt   ask once and exit; otherwise starts a chat (Ctrl-D to quit)";

fn build_provider(name: &str, model: Option<String>) -> Result<Box<dyn Provider>> {
    Ok(match name {
        "openrouter" => {
            let key = std::env::var("OPENROUTER_API_KEY").map_err(|_| "OPENROUTER_API_KEY is not set")?;
            Box::new(OpenAiCompatible::new(
                "https://openrouter.ai/api/v1",
                Some(key),
                model.unwrap_or_else(|| "openrouter/auto".into()),
            ))
        }
        "local" => {
            let base = std::env::var("LOCAL_BASE_URL").unwrap_or_else(|_| "http://localhost:1234/v1".into());
            let model = match model {
                Some(m) => m,
                None => openai::first_chat_model(&base)?,
            };
            Box::new(OpenAiCompatible::new(base, None, model))
        }
        "claude" => Box::new(CliProvider::claude(model)),
        "codex" => Box::new(CliProvider::codex(model)),
        other => return Err(format!("unknown provider `{other}`\n\n{USAGE}").into()),
    })
}

fn chat(provider: &dyn Provider) -> Result<()> {
    let mut history = Vec::new();
    let stdin = io::stdin();
    loop {
        print!("> ");
        io::stdout().flush()?;
        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            return Ok(()); // EOF
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        history.push(Message { role: Role::User, content: line.to_string() });
        match provider.complete(&history) {
            Ok(reply) => {
                println!("{reply}\n");
                history.push(Message { role: Role::Assistant, content: reply });
            }
            Err(e) => {
                eprintln!("error: {e}\n");
                history.pop(); // drop the failed turn so the user can retry
            }
        }
    }
}

fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut positional = Vec::new();
    let mut prompt = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "-p" => prompt = Some(args.next().ok_or("-p needs a prompt")?),
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            _ => positional.push(a),
        }
    }
    let mut positional = positional.into_iter();
    let name = positional.next().ok_or(USAGE)?;
    let provider = build_provider(&name, positional.next())?;

    match prompt {
        Some(p) => {
            let reply = provider.complete(&[Message { role: Role::User, content: p }])?;
            println!("{reply}");
            Ok(())
        }
        None => chat(provider.as_ref()),
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
