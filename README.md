# uriTheKing 👑

The smallest model harness: send a prompt, print the reply, or open an interactive chat.

```bash
cargo install --path crates/cli   # installs `uritheking` into ~/.cargo/bin
uritheking                     # opens the chat (claude backend), type /help
uritheking local -p "hello"
uritheking openrouter anthropic/claude-sonnet-4.5   # chat, Ctrl-D to quit
uritheking claude -p "hello"
uritheking codex gpt-5 -p "hello"
```

| Provider     | How it talks                         | Auth                      |
|--------------|--------------------------------------|---------------------------|
| `openrouter` | HTTP `/chat/completions`             | `OPENROUTER_API_KEY`      |
| `local`      | HTTP `/chat/completions`             | none (`LOCAL_BASE_URL`)   |
| `claude`     | runs `claude -p <prompt>`            | your Claude Code login    |
| `codex`      | runs `codex exec -s read-only ...`   | your Codex login          |

## Layout

A Cargo workspace with two crates:

- `crates/core` (`uritheking-core`, library) – talks to models; no terminal code.
  - `provider.rs` – `Provider` trait, `Message`, `Selection`.
  - `providers/mod.rs` – `build_provider`: picks the backend.
  - `providers/openai.rs` – any OpenAI-compatible server (openrouter, local).
  - `providers/cli.rs` – shells out to an agent CLI (claude, codex).
  - `catalog.rs` – which models each backend offers and their effort levels.
- `crates/cli` (`uritheking`, binary) – the terminal app.
  - `main.rs` – argument parsing.
  - `repl.rs` – chat session: banner, slash commands, `!` shell.
  - `picker.rs` – arrow-key menus for `/model`.
  - `intro.rs` – startup crown animation.

Adding a backend = one struct implementing `Provider` in `crates/core/src/providers/`,
plus one arm in `build_provider` and one in `catalog::models_for`.

## Behavior

#### Scenario: One-shot prompt

- **GIVEN** a configured provider
- **WHEN** the user runs `harness <provider> -p "text"`
- **THEN** the reply is printed to stdout and the process exits 0
- **AND** on any failure the error goes to stderr and the exit code is 1

#### Scenario: Chat keeps history

- **GIVEN** chat mode (no `-p`)
- **WHEN** the user sends several lines
- **THEN** every request includes all earlier user and assistant turns
- **AND** a failed turn is dropped from history so the user can retry it

#### Scenario: CLI providers get flattened history

- **GIVEN** the `claude` or `codex` provider
- **WHEN** there is more than one message in history
- **THEN** the history is sent as a single `User:/Assistant:` transcript prompt

#### Scenario: Local model auto-detect

- **GIVEN** the `local` provider with no model argument
- **WHEN** the harness starts
- **THEN** it uses the first non-embedding model from `<LOCAL_BASE_URL>/models`
- **AND** it exits with an error if the server is down or lists no chat model

#### Scenario: Server error

- **GIVEN** an HTTP provider
- **WHEN** the server answers with a non-2xx status
- **THEN** the status code and response body are shown (e.g. bad key, unknown model)

#### Scenario: Open uriTheKing

- **GIVEN** `uritheking` is installed
- **WHEN** the user runs `uritheking` with no arguments
- **THEN** a crown banner is shown and a chat opens on the `claude` backend
- **AND** each line typed is sent to the model with the full conversation

#### Scenario: Slash commands and shell

- **GIVEN** an open uriTheKing chat
- **WHEN** the user types `/help`, `/model <backend> [model] [effort]`, `/clear`, `/exit` or `!<command>`
- **THEN** the command runs locally and is never sent to the model
- **AND** an unknown `/command` prints a hint to use `/help`; a failed `/model` switch keeps the old backend

#### Scenario: Pick a model and effort

- **GIVEN** an open uriTheKing chat
- **WHEN** the user types `/model` alone
- **THEN** arrow-key menus ask for backend, then model, then effort (`default` first)
- **AND** models come from a built-in list (claude), `~/.codex/models_cache.json` (codex), `<LOCAL_BASE_URL>/models` (local) or OpenRouter's public list
- **AND** the effort menu lists only that model's levels and is skipped when it has none
- **AND** Esc keeps the current choice; a switch keeps the conversation

#### Scenario: Effort reaches the backend

- **GIVEN** an effort level is chosen (picker or `--effort`)
- **THEN** claude gets `--effort <level>`, codex gets `-c model_reasoning_effort="<level>"`
- **AND** openrouter gets `"reasoning": {"effort": ...}`, local gets `"reasoning_effort": ...`

#### Scenario: Startup crown animation

- **GIVEN** `uritheking` is opened in a real terminal (no `-p`)
- **WHEN** it starts
- **THEN** an ASCII crown hops in from the right three times, squashes on landing and sparkles (~2s)
- **AND** the animation is erased and the normal banner prints in its place
- **AND** it is skipped when output is piped or `URITHEKING_NO_INTRO` is set

## Company tools, skills and permissions

Everything lives next to the binary's working dir (or in `URITHEKING_HOME`):

| Path | What it is |
|---|---|
| `COMPANY.md` | What the company and product are. Sent with every call. |
| `tools/<name>/tool.json` | A company tool: `name`, `description`, `params`, `command`. The command runs in the tool's folder with arguments as JSON on stdin and as `ARG_<NAME>` env vars. Whatever it prints is the result. Any language works. |
| `skills/<name>/SKILL.md` | A playbook (frontmatter: `name`, `description`). The model loads it when it fits the task. |
| `permissions.json` | Which tools and skills each role gets, file rules, and the approval mode. |

Built-in tools: `read_file`, `write_file`, `load_skill`.

**Permissions.** Choose a role with `--role sales` or `URITHEKING_ROLE=sales`:
- `"mode": "ask"` asks y/N before every tool call. `"yolo"` runs calls without asking.
- `files.deny` globs are never touched, even with approval. `files.approve` globs always need a manual yes, even in yolo. Paths outside the project are always denied.
- If roles are defined, a missing or unknown role gets no tools. If `permissions.json` is broken, all tools are turned off.

In the chat: `/tools`, `/skills`, `/permissions`, `/reload` (re-reads everything after an edit).
