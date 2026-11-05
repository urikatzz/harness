# uriTheKing 👑

The smallest model harness: send a prompt, print the reply, or open an interactive chat.

```bash
cargo install --path .        # installs `uritheking` into ~/.cargo/bin
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

- `src/provider.rs` – `Provider` trait and `Message` type. The only thing `main` depends on.
- `src/openai.rs` – one implementation for every OpenAI-compatible server.
- `src/cli.rs` – one implementation that shells out to an agent CLI.
- `src/main.rs` – argument parsing, provider selection.
- `src/repl.rs` – interactive session: banner, chat, slash commands, `!` shell.
- `src/picker.rs` – arrow-key menus for `/model` (backend → model → effort).
- `src/catalog.rs` – which models each backend offers and their effort levels.

Adding a provider = one struct implementing `Provider` + one line in `build_provider`.

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
