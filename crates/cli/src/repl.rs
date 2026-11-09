//! Interactive uriTheKing session: banner, chat loop, slash commands and `!` shell.

use crate::picker;
use crate::spinner::Spinner;
use crate::input::{resolve, LineEditor, Read};
use crate::Result;
use std::process::Command as Shell;
use uritheking_core::agent::{self, Event};
use uritheking_core::session::{self, Session};
use uritheking_core::{catalog, context, prompt};
use uritheking_core::{Layout, Message, Policy, Provider, Selection, Toolbox};

const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const GOLD: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const RESET: &str = "\x1b[0m";

/// Builds a provider from a backend name and optional model (injected so the REPL stays backend-agnostic).
pub type ProviderFactory = fn(&Selection) -> uritheking_core::Result<Box<dyn Provider>>;

/// What one line of user input asks for.
enum Input<'a> {
    Chat(&'a str),
    Shell(&'a str),
    Help,
    Clear,
    Exit,
    Tools,
    Skills,
    Reload,
    Permissions,
    Compact,
    /// `/resume` alone opens a picker; `/resume <id>` loads that session.
    Resume(Option<&'a str>),
    /// `/model` alone opens the picker; `/model <backend> [model] [effort]` switches directly.
    Model(Vec<&'a str>),
    Unknown(&'a str),
}

impl<'a> Input<'a> {
    fn parse(line: &'a str) -> Self {
        if let Some(cmd) = line.strip_prefix('!') {
            return Input::Shell(cmd.trim());
        }
        let Some(rest) = line.strip_prefix('/') else {
            return Input::Chat(line);
        };
        let mut parts = rest.split_whitespace();
        match resolve(parts.next().unwrap_or("")) {
            "help" | "?" => Input::Help,
            "clear" | "new" => Input::Clear,
            "exit" | "quit" | "q" => Input::Exit,
            "model" => Input::Model(parts.collect()),
            "tools" => Input::Tools,
            "skills" => Input::Skills,
            "reload" => Input::Reload,
            "permissions" => Input::Permissions,
            "compact" => Input::Compact,
            "resume" => Input::Resume(parts.next()),
            other => Input::Unknown(other),
        }
    }
}

pub struct Repl {
    factory: ProviderFactory,
    provider: Box<dyn Provider>,
    selection: Selection,
    history: Vec<Message>,
    toolbox: Toolbox,
    policy: Policy,
    role: Option<String>,
    /// Project, company setup, this user's data and the shared memory repo.
    layout: Layout,
    session: Session,
}

impl Repl {
    /// `resume_latest` reopens the newest saved session (`--continue`).
    pub fn new(factory: ProviderFactory, selection: Selection, role: Option<String>, resume_latest: bool) -> Result<Self> {
        let provider = factory(&selection)?;
        let layout = uritheking_core::layout().clone();
        let session = Session::new(&layout.data);
        let mut repl = Self { factory, provider, selection, history: Vec::new(), toolbox: Toolbox::default(), policy: Policy::default(), role, layout, session };
        repl.reload();
        if resume_latest {
            match session::list(&repl.layout.data).first() {
                Some(latest) => repl.resume(&latest.id.clone()),
                None => println!("{DIM}no saved sessions yet — starting fresh{RESET}"),
            }
        }
        Ok(repl)
    }

    /// Loads a saved session behind a fresh system prompt; later turns save back to it.
    fn resume(&mut self, id: &str) {
        let session = Session::open(&self.layout.data, id);
        match session.load() {
            Ok(msgs) => {
                self.reset_history();
                let n = msgs.len();
                self.history.extend(msgs);
                self.session = session;
                println!("{DIM}resumed session {id} ({n} messages){RESET}\n");
            }
            Err(e) => println!("{RED}can't resume {id}: {e}{RESET}\n"),
        }
    }

    fn resume_command(&mut self, id: Option<&str>) {
        if let Some(id) = id {
            return self.resume(id);
        }
        let sessions = session::list(&self.layout.data);
        if sessions.is_empty() {
            return println!("{DIM}no saved sessions{RESET}\n");
        }
        let labels: Vec<String> = sessions.iter().map(|s| format!("{}  {} ({} msgs)", s.id, s.preview, s.messages)).collect();
        match inquire::Select::new("resume which session?", labels.clone()).prompt_skippable() {
            Ok(Some(choice)) => {
                let i = labels.iter().position(|l| *l == choice).unwrap_or(0);
                self.resume(&sessions[i].id.clone());
            }
            Ok(None) => println!(),
            Err(e) => println!("{RED}error: {e}{RESET}\n"),
        }
    }

    fn compact_command(&mut self) {
        let before = context::estimate_tokens(&self.history);
        match context::compact(self.provider.as_ref(), &mut self.history) {
            Ok(true) => {
                println!("{DIM}summarized: ~{before} → ~{} tokens{RESET}\n", context::estimate_tokens(&self.history));
                self.save();
            }
            Ok(false) => println!("{DIM}nothing to summarize yet{RESET}\n"),
            Err(e) => println!("{RED}summarize failed: {e}{RESET}\n"),
        }
    }

    fn save(&self) {
        if let Err(e) = self.session.save(&self.history) {
            println!("{RED}couldn't save session: {e}{RESET}");
        }
    }

    /// Re-reads tools/ and skills/ and restarts the conversation with the new instructions.
    fn reload(&mut self) {
        self.toolbox = Toolbox::open(self.layout.clone());
        match Policy::load_with(&self.layout.company, &self.layout.project, self.role.clone()) {
            Ok(p) => self.policy = p,
            // Fail closed: a broken permissions file must not mean "allow everything".
            Err(e) => {
                println!("{RED}permissions: {e} — tools disabled until fixed{RESET}");
                self.policy = Policy::locked(self.role.clone());
            }
        }
        for w in &self.toolbox.warnings {
            println!("{RED}skipped {w}{RESET}");
        }
        self.warn_if_no_file_tools();
        self.reset_history();
    }

    /// Points at the roles that do have file access, so the user knows what to restart with.
    fn warn_if_no_file_tools(&self) {
        if self.policy.can_use_tool("read_file") || self.policy.can_use_tool("write_file") {
            return;
        }
        let mut roles = self.policy.roles_with_tool("write_file");
        if roles.is_empty() { roles = self.policy.roles_with_tool("read_file"); }
        let hint = match roles.as_slice() {
            [] => "no configured role has them; see permissions.json".to_string(),
            roles => format!("restart with --role {}", roles.join(" | ")),
        };
        println!("{GOLD}⚠ File tools unavailable for this role. Check /permissions; {hint}.{RESET}");
    }

    fn reset_history(&mut self) {
        self.history = prompt::conversation(&self.toolbox, &self.policy);
    }

    pub fn run(&mut self) -> Result<()> {
        // Save the current tab title, show ours, and put the old one back on exit.
        print!("\x1b[22;0t\x1b]0;👑 uriTheKing\x07");
        let result = self.run_loop();
        print!("\x1b[23;0t");
        let _ = std::io::Write::flush(&mut std::io::stdout());
        result
    }

    fn run_loop(&mut self) -> Result<()> {
        self.print_banner();
        let mut editor = LineEditor::new();
        loop {
            let line = match editor.read_line(&format!("{GOLD}👑 ›{RESET} "), 5)? {
                Read::Line(line) => line,
                Read::Eof => return Ok(()),
            };
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match Input::parse(line) {
                Input::Exit => return Ok(()),
                Input::Help => self.print_help(),
                Input::Tools => self.print_tools(),
                Input::Permissions => self.print_permissions(),
                Input::Skills => self.print_skills(),
                Input::Reload => {
                    self.reload();
                    println!("{DIM}loaded {} tools, {} skills{RESET}\n", self.toolbox.custom_tools().len(), self.toolbox.skills.len());
                }
                Input::Compact => self.compact_command(),
                Input::Resume(id) => self.resume_command(id),
                Input::Clear => {
                    self.reset_history();
                    self.session = Session::new(&self.layout.data);
                    // Wipe screen and scrollback, then start over like a fresh launch.
                    print!("\x1b[2J\x1b[3J\x1b[H");
                    self.print_banner();
                }
                Input::Model(args) => self.model_command(&args),
                Input::Shell(cmd) => run_shell(cmd),
                Input::Unknown(cmd) => println!("{RED}unknown command /{cmd}{RESET} — try /help\n"),
                Input::Chat(text) => self.chat(text),
            }
        }
    }

    fn chat(&mut self, text: &str) {
        self.history.push(Message::user(text));
        let mut spinner: Option<Spinner> = None;
        let result = agent::run_turn(self.provider.as_ref(), &self.toolbox, &self.policy, &mut self.history, |ev| {
            // Any event means the model answered: clear the spinner before printing.
            spinner = None;
            match ev {
                Event::Thinking => spinner = Some(Spinner::start("thinking")),
                Event::Reasoning { text } => print_reasoning(text),
                Event::ToolCall { name, args } => println!("{DIM}⚙ {name} {args}{RESET}"),
                Event::ToolResult { output, ok, .. } => {
                    let first = output.lines().next().unwrap_or("");
                    let color = if ok { DIM } else { RED };
                    println!("{color}  ↳ {first}{}{RESET}", if output.lines().count() > 1 { " …" } else { "" });
                }
                Event::Denied { reason, .. } => println!("{RED}  ✗ {reason}{RESET}"),
                Event::BadCall { error } => println!("{DIM}  ↻ malformed tool call ({error}); asking the model to resend{RESET}"),
                Event::Compacted { before, after } => println!("{DIM}⇣ summarized older messages: ~{before} → ~{after} tokens{RESET}"),
                Event::CompactFailed { error } => println!("{RED}  summarize failed: {error}{RESET}"),
                Event::Verify { ok, output } => {
                    let first = output.lines().next().unwrap_or("");
                    if ok { println!("{DIM}✓ verified{RESET}") } else { println!("{RED}✗ verify failed: {first}{RESET}") }
                }
                Event::AuditFailed { error } => println!("{RED}  audit log: {error}{RESET}"),
            }
        }, ask_approval);
        drop(spinner);
        match result {
            Ok(reply) => {
                println!("{reply}\n");
                self.save();
            }
            Err(e) => {
                println!("{RED}error: {e}{RESET}\n");
                self.history.push(Message::assistant(format!(
                    "Turn interrupted: {e}. Earlier tool calls may already have completed; inspect their results before retrying."
                )));
                self.save();
            }
        }
    }

    fn print_tools(&self) {
        for t in self.toolbox.available(&self.policy) {
            println!("  {BOLD}{}{RESET}  {DIM}{}{RESET}", t.name(), t.description());
        }
        if self.toolbox.custom_tools().is_empty() {
            println!("{DIM}no company tools — add tools/<name>/tool.json under {}{RESET}", self.layout.company.display());
        }
        println!();
    }

    fn print_permissions(&self) {
        let p = &self.policy;
        println!("  {BOLD}role{RESET}  {}", p.role.as_deref().unwrap_or("none (set --role or URITHEKING_ROLE)"));
        let mode = match p.mode {
            uritheking_core::permissions::Mode::Ask => "ask — confirm each tool call",
            uritheking_core::permissions::Mode::Changes => "changes — confirm calls that change files or use the network",
            uritheking_core::permissions::Mode::Yolo => "yolo — tools run without asking",
        };
        println!("  {BOLD}mode{RESET}  {mode}");
        let available: Vec<&str> = self.toolbox.available(p).map(|t| t.name()).collect();
        for t in self.toolbox.tools() {
            let ok = available.contains(&t.name());
            println!("    {}{} {}{RESET}", if ok { "" } else { DIM }, if ok { "✓" } else { "✗" }, t.name());
        }
        println!("  {DIM}rules: permissions.json in {}{RESET}\n", self.layout.company.display());
    }

    fn print_skills(&self) {
        if self.toolbox.skills.is_empty() {
            println!("{DIM}no skills — add skills/<name>/SKILL.md under {}{RESET}\n", self.layout.company.display());
            return;
        }
        for s in self.toolbox.allowed_skills(&self.policy) {
            println!("  {BOLD}{}{RESET}  {DIM}{}{RESET}", s.name, s.description);
        }
        println!();
    }

    fn model_command(&mut self, args: &[&str]) {
        let chosen = match args {
            [] => picker::pick(&self.selection),
            [backend, rest @ ..] => Ok(Some(Selection {
                backend: backend.to_string(),
                model: rest.first().map(|m| m.to_string()),
                effort: rest.get(1).map(|e| e.to_string()),
            })),
        };
        match chosen {
            Ok(Some(sel)) => self.switch_to(sel),
            Ok(None) => println!("{DIM}kept {}{RESET}\n", self.label()),
            Err(e) => println!("{RED}error: {e}{RESET}\n"),
        }
    }

    /// Swaps the provider but keeps the conversation; on failure the old one stays.
    fn switch_to(&mut self, selection: Selection) {
        match (self.factory)(&selection) {
            Ok(p) => {
                self.provider = p;
                self.selection = selection;
                println!("{DIM}switched to{RESET} {}", self.label());
                self.warn_if_unloaded();
                println!();
            }
            Err(e) => println!("{RED}error: {e}{RESET}\n"),
        }
    }

    fn label(&self) -> String {
        let s = &self.selection;
        format!(
            "{BOLD}{}{RESET} {DIM}·{RESET} {} {DIM}· effort{RESET} {}",
            s.backend,
            s.model.as_deref().unwrap_or("default model"),
            s.effort.as_deref().unwrap_or("default")
        )
    }

    fn print_banner(&self) {
        println!();
        println!("  {GOLD}👑{RESET}  {BOLD}uriTheKing{RESET} {DIM}v{}{RESET}", env!("CARGO_PKG_VERSION"));
        println!(
            "      {DIM}/ commands ·{RESET} ! {DIM}bash ·{RESET} ctrl+d {DIM}exit{RESET}"
        );
        println!("      {}", self.label());
        println!("  {DIM}Type /help to see what uriTheKing can do.{RESET}");
        self.warn_if_unloaded();
        println!();
    }

    fn warn_if_unloaded(&self) {
        if self.selection.backend == "local" {
            if let Some(w) = catalog::local_load_warning(self.selection.model.as_deref()) {
                println!("{GOLD}⚠ {w}{RESET}");
            }
        }
    }

    fn print_help(&self) {
        println!(
            "\
{GOLD}{BOLD}What uriTheKing can do{RESET}

  {BOLD}just type{RESET}                   chat with the model (it remembers the conversation,
                              also across /model switches)
  {BOLD}/help{RESET}                       show this help
  {BOLD}/model{RESET}                      pick backend → model → effort with the arrow keys
  {BOLD}/model <backend> [model] [effort]{RESET}  switch directly, e.g. /model codex gpt-6-luna high
  {BOLD}/tools{RESET}                      list company tools (tools/<name>/tool.json)
  {BOLD}/skills{RESET}                     list skills (skills/<name>/SKILL.md)
  {BOLD}/permissions{RESET}                 show your role, approval mode and allowed tools
  {BOLD}/reload{RESET}                     re-read tools/ and skills/ after editing them
  {BOLD}/clear{RESET}                      forget the conversation and start a new session
  {BOLD}/resume [id]{RESET}                pick a saved session (every chat autosaves to sessions/)
  {BOLD}/compact{RESET}                    summarize older messages to free up context
                              (happens on its own past ~URITHEKING_CONTEXT_TOKENS, default 60000)
  {BOLD}!<command>{RESET}                  run a shell command, e.g. !ls
  {BOLD}/exit{RESET} or ctrl+d             quit

{GOLD}{BOLD}Backends{RESET}
  claude       your Claude Code login
  codex        your Codex login (read-only)
  local        LM Studio / Ollama (LOCAL_BASE_URL)
  openrouter   needs OPENROUTER_API_KEY
"
        );
    }
}

/// Runs `cmd` through the user's shell, streaming its output straight to the terminal.
/// Most of a model's reasoning shown dimmed above its answer; very long ones are cut.
fn print_reasoning(text: &str) {
    const MAX_LINES: usize = 30;
    let lines: Vec<&str> = text.lines().collect();
    println!("{DIM}✻ reasoning{RESET}");
    for line in lines.iter().take(MAX_LINES) {
        println!("{DIM}│ {line}{RESET}");
    }
    if lines.len() > MAX_LINES {
        println!("{DIM}│ … {} more lines{RESET}", lines.len() - MAX_LINES);
    }
    println!();
}

fn run_shell(cmd: &str) {
    if cmd.is_empty() {
        println!("{DIM}usage: !<command>{RESET}\n");
        return;
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    match Shell::new(shell).arg("-c").arg(cmd).status() {
        Ok(status) if !status.success() => println!("{DIM}exit {}{RESET}\n", status.code().unwrap_or(-1)),
        Ok(_) => println!(),
        Err(e) => println!("{RED}error: {e}{RESET}\n"),
    }
}

/// Asks the user y/N before a tool call runs. Anything but y/yes is a no.
pub fn ask_approval(name: &str, args: &serde_json::Value, why: &str) -> bool {
    use std::io::{BufRead, IsTerminal, Write};
    if !std::io::stdin().is_terminal() {
        eprintln!("{RED}  ✗ {name} needs approval ({why}) but there is no terminal to ask{RESET}");
        return false;
    }
    eprint!("{GOLD}  ? allow {BOLD}{name}{RESET}{GOLD} {args} — {why}? [y/N] {RESET}");
    std::io::stderr().flush().ok();
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer).ok();
    matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct FailsAfterWrite(Cell<usize>);
    impl Provider for FailsAfterWrite {
        fn complete(&self, _: &[Message]) -> uritheking_core::Result<String> {
            let call = self.0.get();
            self.0.set(call + 1);
            if call == 0 {
                Ok(r#"<tool_call>{"name":"write_file","args":{"path":"result.txt","content":"done"}}</tool_call>"#.into())
            } else { Err("inference unavailable".into()) }
        }
    }

    #[test]
    fn failed_turn_retains_completed_tool_results_in_saved_session() {
        let id = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let home = std::env::temp_dir().join(format!("uritheking-recovery-{}-{id}", std::process::id()));
        std::fs::create_dir(&home).unwrap();
        std::fs::write(home.join("permissions.json"), r#"{"mode":"yolo"}"#).unwrap();
        let mut repl = Repl {
            factory: |_| unreachable!(),
            provider: Box::new(FailsAfterWrite(Cell::new(0))),
            selection: Selection::default(),
            history: vec![],
            toolbox: Toolbox::load(&home),
            policy: Policy::load(&home, None).unwrap(),
            role: None,
            layout: Layout::single(&home),
            session: Session::new(&home),
        };
        repl.chat("write the result");
        assert_eq!(std::fs::read_to_string(home.join("result.txt")).unwrap(), "done");
        let saved = repl.session.load().unwrap();
        assert!(saved.iter().any(|m| m.content.contains("wrote 4 bytes")));
        assert!(saved.last().unwrap().content.contains("Turn interrupted"));
        std::fs::remove_dir_all(home).unwrap();
    }
}
