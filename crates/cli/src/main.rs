mod input;
mod intro;
mod picker;
mod repl;

use repl::Repl;
use uritheking_core::{agent, build_provider, toolbox, Message, Result, Role, Policy, Selection, Toolbox};

const DEFAULT_BACKEND: &str = "claude";

const USAGE: &str = "usage: uritheking [openrouter|local|claude|codex] [model] [--effort level] [--role name] [-p prompt]

  no arguments  opens the uriTheKing chat with the claude backend (type /help inside)

  openrouter  needs OPENROUTER_API_KEY            (default model: openrouter/auto)
  local       OpenAI-compatible server, LOCAL_BASE_URL (default http://localhost:1234/v1)
              (default model: first one the server lists; Ollama: http://localhost:11434/v1)
  claude      runs `claude -p`, uses your Claude Code login
  codex       runs `codex exec`, uses your Codex login

  --effort    reasoning effort, e.g. low | medium | high (levels depend on the model)
  --role      your team, e.g. sales | art (or URITHEKING_ROLE); decides which tools you get
              (rules live in permissions.json)
  -p prompt   ask once and exit; otherwise opens the chat";

fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut positional = Vec::new();
    let mut prompt = None;
    let mut effort = None;
    let mut role = std::env::var("URITHEKING_ROLE").ok();
    while let Some(a) = args.next() {
        match a.as_str() {
            "-p" => prompt = Some(args.next().ok_or("-p needs a prompt")?),
            "--role" => role = Some(args.next().ok_or("--role needs a name")?),
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
            let tb = Toolbox::load(&toolbox::home());
            let policy = Policy::load(&toolbox::home(), role)?;
            let mut history = Vec::new();
            let system = agent::system_prompt(&tb, &policy);
            if !system.is_empty() {
                history.push(Message { role: Role::System, content: system });
            }
            history.push(Message { role: Role::User, content: p });
            let reply = agent::run_turn(build_provider(&selection)?.as_ref(), &tb, &policy, &mut history, |ev| {
                if let agent::Event::ToolCall { name, args } = ev {
                    eprintln!("⚙ {name} {args}");
                }
                if let agent::Event::Denied { reason, .. } = ev {
                    eprintln!("✗ {reason}");
                }
            }, repl::ask_approval)?;
            println!("{reply}");
            Ok(())
        }
        None => {
            intro::Intro::default().play_if_interactive();
            Repl::new(build_provider, selection, role)?.run()
        }
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
