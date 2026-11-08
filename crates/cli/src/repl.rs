//! Interactive uriTheKing session: banner, chat loop, slash commands and `!` shell.

use crate::picker;
use uritheking_core::agent::{self, Event};
use uritheking_core::context;
use uritheking_core::session::{self, Session};
use uritheking_core::{toolbox, Message, Provider, Result, Role, Policy, Selection, Toolbox};
use crate::input::{resolve, LineEditor, Read};
use std::process::Command as Shell;

const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const GOLD: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const RESET: &str = "\x1b[0m";

/// Builds a provider from a backend name and optional model (injected so the REPL stays backend-agnostic).
pub type ProviderFactory = fn(&Selection) -> Result<Box<dyn Provider>>;

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
    session: Session,
}

impl Repl {
    /// `resume_latest` reopens the newest saved session (`--continue`).
    pub fn new(factory: ProviderFactory, selection: Selection, role: Option<String>, resume_latest: bool) -> Result<Self> {
        let provider = factory(&selection)?;
        let session = Session::new(&toolbox::home());
        let mut repl = Self { factory, provider, selection, history: Vec::new(), toolbox: Toolbox::default(), policy: Policy::default(), role, session };
        repl.reload();
        if resume_latest {
            match session::list(&toolbox::home()).first() {
                Some(latest) => repl.resume(&latest.id.clone()),
                None => println!("{DIM}no saved sessions yet — starting fresh{RESET}"),
            }
        }
        Ok(repl)
    }

    /// Loads a saved session behind a fresh system prompt; later turns save back to it.
    fn resume(&mut self, id: &str) {
        let session = Session::open(&toolbox::home(), id);
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
        let sessions = session::list(&toolbox::home());
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
        self.toolbox = Toolbox::load(&toolbox::home());
        match Policy::load(&toolbox::home(), self.role.clone()) {
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
        self.reset_history();
    }

    fn reset_history(&mut self) {
        self.history.clear();
        let system = agent::system_prompt(&self.toolbox, &self.policy);
        if !system.is_empty() {
            self.history.push(Message { role: Role::System, content: system });
        }
    }

    pub fn run(&mut self) -> Result<()> {
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
                    println!("{DIM}loaded {} tools, {} skills{RESET}\n", self.toolbox.tools.len(), self.toolbox.skills.len());
                }
                Input::Compact => self.compact_command(),
                Input::Resume(id) => self.resume_command(id),
                Input::Clear => {
                    self.reset_history();
                    self.session = Session::new(&toolbox::home());
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
        self.history.push(Message { role: Role::User, content: text.to_string() });
        let result = agent::run_turn(self.provider.as_ref(), &self.toolbox, &self.policy, &mut self.history, |ev| match ev {
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
        }, ask_approval);
        match result {
            Ok(reply) => {
                println!("{reply}\n");
                self.save();
            }
            Err(e) => {
                println!("{RED}error: {e}{RESET}\n");
                self.history.push(Message {
                    role: Role::Assistant,
                    content: format!("Turn interrupted: {e}. Earlier tool calls may already have completed; inspect their results before retrying."),
                });
                self.save();
            }
        }
    }

    fn print_tools(&self) {
        if self.toolbox.tools.is_empty() {
            println!("{DIM}no tools — add tools/<name>/tool.json under {}{RESET}\n", toolbox::home().display());
            return;
        }
        for t in self.toolbox.tools.iter().filter(|t| self.policy.can_use_tool(&t.name)) {
            println!("  {BOLD}{}{RESET}  {DIM}{}{RESET}", t.name, t.description);
        }
        println!();
    }

    fn print_permissions(&self) {
        let p = &self.policy;
        println!("  {BOLD}role{RESET}  {}", p.role.as_deref().unwrap_or("none (set --role or URITHEKING_ROLE)"));
        println!("  {BOLD}mode{RESET}  {}", if p.yolo { "yolo — tools run without asking" } else { "ask — confirm each tool call" });
        let tools: Vec<&str> = toolbox::BUILTINS.iter().map(|b| b.0).chain(self.toolbox.tools.iter().map(|t| t.name.as_str())).collect();
        for t in tools {
            let ok = p.can_use_tool(t);
            println!("    {}{} {t}{RESET}", if ok { "" } else { DIM }, if ok { "✓" } else { "✗" });
        }
        println!("  {DIM}rules: permissions.json in {}{RESET}\n", toolbox::home().display());
    }

    fn print_skills(&self) {
        if self.toolbox.skills.is_empty() {
            println!("{DIM}no skills — add skills/<name>/SKILL.md under {}{RESET}\n", toolbox::home().display());
            return;
        }
        for s in self.toolbox.skills.iter().filter(|s| self.policy.can_use_skill(&s.name)) {
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
                println!("{DIM}switched to{RESET} {}\n", self.label());
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
        println!("  {DIM}Type /help to see what uriTheKing can do.{RESET}\n");
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
        fn complete(&self, _: &[Message]) -> Result<String> {
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
