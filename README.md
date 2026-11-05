# harness

The smallest model harness: send a prompt, print the reply. Supports chat mode with history.

```bash
cargo build --release
./target/release/harness local -p "hello"
./target/release/harness openrouter anthropic/claude-sonnet-4.5   # chat, Ctrl-D to quit
./target/release/harness claude -p "hello"
./target/release/harness codex gpt-5 -p "hello"
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
- `src/main.rs` – argument parsing, provider selection, chat loop.

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
