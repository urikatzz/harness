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
- **AND** failed turns retain completed tool calls and results, record the interruption, and save the session so retries can account for existing changes

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
| `tools/<name>/tool.json` | A company tool: `name`, `description`, `params`, `command`, and optional `reads` (project folders it may read, `"."` = whole project), `writes` (project folders it may write to), `network` (`true` to allow outbound network) and `env` (names of environment variables to pass through). The command runs in the tool's folder with arguments as JSON on stdin and as `ARG_<NAME>` env vars. Whatever it prints is the result. Any language works. |
| `skills/<name>/SKILL.md` | A playbook (frontmatter: `name`, `description`). The model loads it when it fits the task. |
| `permissions.json` | Which tools and skills each role gets, file rules, the approval mode, and the optional `verify` tool. |
| `evals/<name>.json` | Eval cases: a prompt and what should happen. Run with `uritheking eval`. |
| `logs/audit.jsonl` | Written by the harness: every tool call, who made it and what was decided. |

Built-in tools: `read_file`, `write_file`, `load_skill`.

**Permissions.** Choose a role with `--role sales` or `URITHEKING_ROLE=sales`:
- `"mode": "ask"` asks y/N before every tool call. `"yolo"` runs calls without asking.
- `files.deny` globs are never touched, even with approval. `files.approve` globs always need a manual yes, even in yolo. Paths outside the project are always denied.
- Built in, whatever the file says: `write_file` can never touch `sessions/` or `logs/`, and writing `permissions.json`, `COMPANY.md`, `tools/` or `skills/` always needs a manual yes (they control what the model may do).
- **Untrusted output.** File contents and tool output can contain instructions planted by someone else. The model is told to treat `<tool_result>` text as data, and tool output can't fake its own end tag or a tool call. Once any tool output is in the conversation, calls that can change things (`write_file`, tools with `writes` or `network`) need a manual yes even in yolo. Set `"trust_tool_output": true` to turn that off.
- If roles are defined, a missing or unknown role gets no tools. Unreadable, malformed, or incorrectly typed permission files fail closed.
- File tools normalize relative paths, reject traversal, symlinks and hard links, and open files through directory descriptors. Reads are limited to 1 MiB.
- Custom commands run in a filesystem sandbox: they may read their tool directory, system runtimes and their `reads` folders, and read/write a private per-call temp folder (`$TMPDIR`, deleted afterwards) plus the folders listed in their `writes`. Everything else in or outside the project is off-limits, and so are `sessions/` and `logs/`. `writes` entries must be existing project folders below the top level, and never `tools/`, `skills/`, `sessions/` or `logs/`. Denied and approval-required paths stay unavailable to custom commands even inside `reads`/`writes`; use built-in file tools for those.
- Custom commands get no network unless their `tool.json` says `"network": true`, and a clean environment: only `PATH`, `HOME`, `USER`, `LANG`, `LC_ALL`, `LC_CTYPE`, `TZ`, `TMPDIR`, their `ARG_*` values and the variables named in `env`. API keys and tokens are not passed on.
- CLI backends also run in the sandbox, start outside the project, and cannot read or write project contents through their native tools. Their login/configuration access remains available outside the project. HTTP backends do not need the sandbox.
- macOS uses `sandbox-exec` (deprecated by Apple). Linux uses bubblewrap (`bwrap`; install the `bubblewrap` package): an empty read-only root with only system folders and the tool's folders mounted, all capabilities dropped. On Linux, existing protected files are covered with an unreadable placeholder, and protected files a tool creates in its `writes` folders are removed and the call fails. The sandbox is checked once per run; if it is missing or broken, or on Windows, custom tools and CLI backends refuse to run and say why. Set `"sandbox": false` in `permissions.json` to run them without it. They then have your normal file and network access (the environment is still cleaned), so only do this when you trust the tools and backends. Path checks for built-in file tools still apply.
- Custom commands have a 60-second deadline; CLI inference has a 120-second deadline. Both have an 8 MiB input limit and 1 MiB per-stream output limits, and their process groups are terminated on timeout or excess output.
- Tool calls must be a single, complete `<tool_call>` response containing exactly `name` and an object `args`. A malformed or mixed response runs nothing; the model gets the error back and can resend (this counts toward the 10-step limit).

In the chat: `/tools`, `/skills`, `/permissions`, `/reload` (re-reads everything after an edit).

**Try the mock skill and tool.** Start with an authorized role:

```bash
uritheking codex --role dev
# Type: Run the mock demo with the message hello.
```

The `mock-demo` skill calls `mock_echo`, which prints `Mock echo: hello` without accessing
files or the network. In ask mode, approve the tool call when prompted. Use `/reload` in an
existing chat to discover newly added extensions.

When roles are configured, launching without `--role` or `URITHEKING_ROLE` grants no tools.
The harness warns when file tools are unavailable, and tells the model to explain the limitation
instead of promising changes. To create extensions through chat, use a role with `write_file`
access (such as `dev` in the example configuration); each configuration write still requires approval.

The tool loop runs only explicit `<tool_call>` replies. A plain-text reply ends the turn, so
"I'll add it" does not mean files were changed. CLI backends can confuse these text-based
tools with their native tools; run `uritheking eval codex <model> --effort <level>` to check
the selected model. Failed evals show the missing calls and the model's reply.

**Audit log.** Every tool call adds a `decision` line to `logs/audit.jsonl` (time, role, tool, arguments with long strings cut to 200 characters, and `allowed`/`approved`/`declined`/`denied`/`invalid` with the reason) before anything runs, and a `result` line after. If the log can't be written, the call is refused.

**Verify.** Set `"verify": "<tool name>"` in `permissions.json` to run that tool after the model changes files and before its answer is accepted. If it fails, its output goes back to the model to fix; after 3 failures in one turn, the turn stops with an error. The verify tool is set by whoever owns `permissions.json`, so it runs for every role. Give it `reads` so it can see the project.

**Evals.** `uritheking eval [backend] [model] [--role name]` runs every `evals/<name>.json`:

```json
{
  "prompt": "What tech stack does the company use?",
  "role": "sales",
  "expect": {
    "tools": ["company_info"],
    "no_tools": ["write_file"],
    "reply_contains": ["Rust"],
    "reply_not_contains": ["I can't"]
  }
}
```

Each case runs in a fresh conversation. Read-only calls run; calls that could write files or use the network are recorded and declined, so evals never change anything. It prints pass/fail per case and exits 1 if any fail.
