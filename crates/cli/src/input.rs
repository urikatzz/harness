//! Line editor with a Claude Code–style slash-command menu that opens under the prompt as you type.

use crossterm::cursor::{MoveToColumn, MoveUp};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::style::Print;
use crossterm::terminal::{self, Clear, ClearType};
use crossterm::{execute, queue};
use std::io::{self, Write};

const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const ACCENT: &str = "\x1b[38;5;147m";
const RESET: &str = "\x1b[0m";

/// Slash commands shown in the menu: (name, description).
pub const COMMANDS: &[(&str, &str)] = &[
    ("help", "Show what uriTheKing can do"),
    ("model", "Pick backend, model and effort"),
    ("tools", "List company tools"),
    ("skills", "List skills"),
    ("permissions", "Show role, approval mode and allowed tools"),
    ("reload", "Re-read tools/ and skills/"),
    ("resume", "Reopen a saved session"),
    ("compact", "Summarize older messages to free context"),
    ("clear", "Forget the conversation and start fresh"),
    ("new", "Same as /clear"),
    ("exit", "Quit uriTheKing"),
    ("quit", "Same as /exit"),
];

const MAX_ROWS: usize = 8;

/// Expands an unambiguous prefix (`mo` → `model`); otherwise returns it unchanged.
pub fn resolve(word: &str) -> &str {
    let mut m = COMMANDS.iter().map(|c| c.0).filter(|c| c.starts_with(word));
    match (m.next(), m.next()) {
        _ if COMMANDS.iter().any(|c| c.0 == word) => word,
        (Some(only), None) if !word.is_empty() => only,
        _ => word,
    }
}

pub enum Read {
    Line(String),
    Eof,
}

pub struct LineEditor {
    history: Vec<String>,
}

struct State {
    buf: Vec<char>,
    cursor: usize,
    selected: usize,
    dismissed: bool,
    history_pos: usize,
}

impl State {
    fn text(&self) -> String {
        self.buf.iter().collect()
    }

    /// Menu entries while the user is still typing the command word.
    fn matches(&self) -> Vec<(&'static str, &'static str)> {
        let text = self.text();
        let Some(typed) = text.strip_prefix('/') else { return vec![] };
        if self.dismissed || typed.contains(char::is_whitespace) {
            return vec![];
        }
        COMMANDS.iter().copied().filter(|c| c.0.starts_with(typed)).collect()
    }

    fn set(&mut self, text: &str) {
        self.buf = text.chars().collect();
        self.cursor = self.buf.len();
        self.selected = 0;
    }
}

impl LineEditor {
    pub fn new() -> Self {
        Self { history: Vec::new() }
    }

    /// Reads one line. `prompt` may contain ANSI codes; `prompt_width` is its visible width.
    pub fn read_line(&mut self, prompt: &str, prompt_width: u16) -> io::Result<Read> {
        terminal::enable_raw_mode()?;
        let result = self.read_raw(prompt, prompt_width);
        terminal::disable_raw_mode()?;
        if let Ok(Read::Line(line)) = &result {
            if !line.trim().is_empty() && self.history.last() != Some(line) {
                self.history.push(line.clone());
            }
        }
        result
    }

    fn read_raw(&mut self, prompt: &str, pw: u16) -> io::Result<Read> {
        let mut out = io::stdout();
        let mut s = State {
            buf: vec![],
            cursor: 0,
            selected: 0,
            dismissed: false,
            history_pos: self.history.len(),
        };
        render(&mut out, prompt, pw, &mut s)?;
        loop {
            let Event::Key(KeyEvent { code, modifiers, kind, .. }) = event::read()? else {
                continue;
            };
            if kind == KeyEventKind::Release {
                continue;
            }
            let ctrl = modifiers.contains(KeyModifiers::CONTROL);
            let menu = s.matches();
            match code {
                KeyCode::Char('c') if ctrl => {
                    if s.buf.is_empty() {
                        finish(&mut out, prompt, pw, &mut s)?;
                        return Ok(Read::Eof);
                    }
                    s.set("");
                }
                KeyCode::Char('d') if ctrl && s.buf.is_empty() => {
                    finish(&mut out, prompt, pw, &mut s)?;
                    return Ok(Read::Eof);
                }
                KeyCode::Char('u') if ctrl => {
                    s.buf.drain(..s.cursor);
                    s.cursor = 0;
                }
                KeyCode::Char(c) if !ctrl => {
                    s.buf.insert(s.cursor, c);
                    s.cursor += 1;
                    s.selected = 0;
                    s.dismissed = false;
                }
                KeyCode::Backspace if s.cursor > 0 => {
                    s.cursor -= 1;
                    s.buf.remove(s.cursor);
                    s.selected = 0;
                    s.dismissed = false;
                }
                KeyCode::Delete if s.cursor < s.buf.len() => {
                    s.buf.remove(s.cursor);
                }
                KeyCode::Left => s.cursor = s.cursor.saturating_sub(1),
                KeyCode::Right => s.cursor = (s.cursor + 1).min(s.buf.len()),
                KeyCode::Home => s.cursor = 0,
                KeyCode::End => s.cursor = s.buf.len(),
                KeyCode::Esc => s.dismissed = true,
                KeyCode::Up if !menu.is_empty() => {
                    s.selected = (s.selected + menu.len() - 1) % menu.len();
                }
                KeyCode::Down if !menu.is_empty() => s.selected = (s.selected + 1) % menu.len(),
                KeyCode::Up if s.history_pos > 0 => {
                    s.history_pos -= 1;
                    let h = self.history[s.history_pos].clone();
                    s.set(&h);
                    s.dismissed = true;
                }
                KeyCode::Down if s.history_pos < self.history.len() => {
                    s.history_pos += 1;
                    let h = self.history.get(s.history_pos).cloned().unwrap_or_default();
                    s.set(&h);
                    s.dismissed = true;
                }
                KeyCode::Tab if !menu.is_empty() => {
                    s.set(&format!("/{} ", menu[s.selected].0));
                }
                KeyCode::Enter => {
                    if !menu.is_empty() {
                        s.set(&format!("/{}", menu[s.selected].0));
                    }
                    finish(&mut out, prompt, pw, &mut s)?;
                    return Ok(Read::Line(s.text()));
                }
                _ => {}
            }
            render(&mut out, prompt, pw, &mut s)?;
        }
    }
}

/// Redraws the prompt line and the menu below it, leaving the cursor in the input.
fn render(out: &mut io::Stdout, prompt: &str, pw: u16, s: &mut State) -> io::Result<()> {
    let width = terminal::size().map(|(w, _)| w as usize).unwrap_or(80);
    queue!(out, MoveToColumn(0), Clear(ClearType::FromCursorDown))?;
    queue!(out, Print(prompt), Print(s.text()))?;

    let menu = s.matches();
    let shown = menu.len().min(MAX_ROWS);
    let start = s.selected.saturating_sub(shown.saturating_sub(1));
    let name_col = COMMANDS.iter().map(|c| c.0.len()).max().unwrap_or(0) + 6;
    let typed_len = s.buf.len().saturating_sub(1);
    for (i, (name, desc)) in menu.iter().enumerate().skip(start).take(shown) {
        let sel = i == s.selected;
        let (marker, color) = if sel { ("❯ ", ACCENT) } else { ("  ", DIM) };
        let (hit, rest) = name.split_at(typed_len.min(name.len()));
        let pad = name_col.saturating_sub(name.len() + 3);
        let room = width.saturating_sub(name_col + 2);
        let desc: String = desc.chars().take(room).collect();
        queue!(
            out,
            Print("\r\n"),
            Print(format!(
                "{color}{marker}/{RESET}{color}{BOLD}{hit}{RESET}{color}{rest}{}{desc}{RESET}",
                " ".repeat(pad)
            ))
        )?;
    }
    if shown > 0 {
        queue!(out, MoveUp(shown as u16))?;
    }
    queue!(out, MoveToColumn(pw + s.cursor as u16))?;
    out.flush()
}

/// Clears the menu and moves to a fresh line.
fn finish(out: &mut io::Stdout, prompt: &str, pw: u16, s: &mut State) -> io::Result<()> {
    s.dismissed = true;
    render(out, prompt, pw, s)?;
    execute!(out, Print("\r\n"))
}
