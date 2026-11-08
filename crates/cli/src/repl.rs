//! Interactive uriTheKing session: banner, chat loop, slash commands and `!` shell.

use crate::picker;
use uritheking_core::{Message, Provider, Result, Role, Selection};
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
            other => Input::Unknown(other),
        }
    }
}

pub struct Repl {
    factory: ProviderFactory,
    provider: Box<dyn Provider>,
    selection: Selection,
    history: Vec<Message>,
}

impl Repl {
    pub fn new(factory: ProviderFactory, selection: Selection) -> Result<Self> {
        let provider = factory(&selection)?;
        Ok(Self { factory, provider, selection, history: Vec::new() })
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
                Input::Clear => {
                    self.history.clear();
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
        match self.provider.complete(&self.history) {
            Ok(reply) => {
                println!("{reply}\n");
                self.history.push(Message { role: Role::Assistant, content: reply });
            }
            Err(e) => {
                println!("{RED}error: {e}{RESET}\n");
                self.history.pop(); // drop the failed turn so the user can retry
            }
        }
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
  {BOLD}/clear{RESET}                      forget the conversation and start fresh
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
