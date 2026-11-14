mod input;
mod intro;
mod picker;
mod repl;
mod spinner;

use repl::Repl;
use umango_core::{agent, build_provider, layout, prompt, Message, Policy, Selection, Toolbox};

/// The app reports any error as text, so one boxed type is enough here.
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const DEFAULT_BACKEND: &str = "claude";

const USAGE: &str = "usage: umango [openrouter|local|claude|codex] [model] [--effort level] [--role name] [-c] [-p prompt]
       umango eval [backend] [model] [--effort level] [--role name]

  no arguments  opens the umango chat with the claude backend (type /help inside)

  openrouter  needs OPENROUTER_API_KEY            (default model: openrouter/auto)
  local       OpenAI-compatible server, LOCAL_BASE_URL (default http://localhost:1234/v1)
              (default model: first one the server lists; Ollama: http://localhost:11434/v1)
  claude      runs `claude -p`, uses your Claude Code login
  codex       runs `codex exec`, uses your Codex login

  --effort    reasoning effort, e.g. low | medium | high (levels depend on the model)
  --role      your team, e.g. sales | art (or UMANGO_ROLE); decides which tools you get
              (rules live in permissions.json)
  -c, --continue  reopen the most recent saved session
  -p prompt   ask once and exit; otherwise opens the chat

  eval        runs every evals/<name>.json case and reports pass/fail (exit 1 if any fail).
              Calls that could write files or use the network are declined, never run.";

fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut positional = Vec::new();
    let mut prompt = None;
    let mut effort = None;
    let mut resume = false;
    let mut role = std::env::var("UMANGO_ROLE").ok();
    while let Some(a) = args.next() {
        match a.as_str() {
            "-p" => prompt = Some(args.next().ok_or("-p needs a prompt")?),
            "--role" => role = Some(args.next().ok_or("--role needs a name")?),
            "--effort" => effort = Some(args.next().ok_or("--effort needs a level")?),
            "-c" | "--continue" => resume = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            _ => positional.push(a),
        }
    }
    let eval = positional.first().is_some_and(|p| p == "eval");
    if eval { positional.remove(0); }
    let mut positional = positional.into_iter();
    let selection = Selection {
        backend: positional.next().unwrap_or_else(|| DEFAULT_BACKEND.into()),
        model: positional.next(),
        effort,
    };

    if eval {
        return run_evals(&selection, role);
    }
    match prompt {
        Some(p) => {
            let (layout, tb) = (layout(), Toolbox::open(layout().clone()));
            let policy = Policy::load_with(&layout.company, &layout.project, role)?;
            let mut history = prompt::conversation(&tb, &policy);
            history.push(Message::user(p));
            let reply = agent::run_turn(build_provider(&selection)?.as_ref(), &tb, &policy, &mut history, |ev| {
                if let agent::Event::ToolCall { name, args } = ev {
                    eprintln!("⚙ {name} {args}");
                }
                if let agent::Event::Denied { reason, .. } = ev {
                    eprintln!("✗ {reason}");
                }
                if let agent::Event::BadCall { error } = ev {
                    eprintln!("↻ malformed tool call ({error}); asking the model to resend");
                }
                if let agent::Event::Verify { ok, output } = ev {
                    eprintln!("{} {}", if ok { "✓ verified" } else { "✗ verify failed:" }, if ok { "" } else { output.lines().next().unwrap_or("") });
                }
                if let agent::Event::AuditFailed { error } = ev {
                    eprintln!("audit log: {error}");
                }
            }, repl::ask_approval)?;
            println!("{reply}");
            Ok(())
        }
        None => {
            intro::Intro::default().play_if_interactive();
            Repl::new(build_provider, selection, role, resume)?.run()
        }
    }
}

fn run_evals(selection: &Selection, role: Option<String>) -> Result<()> {
    use umango_core::eval;
    let layout = layout();
    let tb = Toolbox::open(layout.clone());
    let (cases, errors) = eval::load(&layout.company);
    for e in &errors { eprintln!("✗ {e}"); }
    if cases.is_empty() {
        return Err(format!("no eval cases in {}", layout.company.join("evals").display()).into());
    }
    let provider = build_provider(selection)?;
    let mut failed = errors.len();
    for case in &cases {
        let policy = Policy::load_with(&layout.company, &layout.project, case.role.clone().or_else(|| role.clone()))?;
        let outcome = eval::run(provider.as_ref(), &tb, &policy, case);
        if outcome.passed() {
            println!("✓ {}", outcome.name);
        } else {
            failed += 1;
            println!("✗ {}", outcome.name);
            for f in &outcome.failures { println!("    {f}"); }
            println!("    tools called: {}", if outcome.tools_called.is_empty() { "none".into() } else { outcome.tools_called.join(", ") });
            println!("    reply: {}", outcome.reply.chars().take(1000).collect::<String>());
        }
    }
    println!("\n{} passed, {failed} failed", cases.len() + errors.len() - failed);
    if failed > 0 { std::process::exit(1); }
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
