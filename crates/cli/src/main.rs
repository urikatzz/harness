mod input;
mod intro;
mod picker;
mod repl;

use repl::Repl;
use uritheking_core::{build_provider, Message, Result, Role, Selection};

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
        None => {
            intro::Intro::default().play_if_interactive();
            Repl::new(build_provider, selection)?.run()
        }
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
