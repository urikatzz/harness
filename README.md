# uriTheKing 👑

The smallest model harness: send a prompt, print the reply, or open an interactive chat.

## Install

macOS or Linux:

```bash
curl -fsSL https://raw.githubusercontent.com/urikatzz/harness/main/install.sh | sh
```

This downloads the latest release build, checks its SHA-256 checksum, and puts `uritheking` in
`~/.local/bin`. Set `URITHEKING_INSTALL_DIR` to install somewhere else, or `URITHEKING_VERSION=v0.1.0`
to pin a version. Run the same command again to update. To build from source instead:
`cargo install --path crates/cli`.

**Releasing:** push a tag like `v0.1.0`. `.github/workflows/release.yml` builds macOS and Linux
binaries (Intel and ARM) and attaches them to the GitHub release the installer reads.

## Use

```bash
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
  - `provider.rs` – `Provider` trait, `Message` (with its `Kind`: chat, tool result, summary), `Selection`.
  - `providers/mod.rs` – `build_provider`: picks the backend.
  - `providers/openai.rs` – any OpenAI-compatible server (openrouter, local).
  - `providers/cli.rs` – shells out to an agent CLI (claude, codex).
  - `catalog.rs` – which models each backend offers and their effort levels.
  - `agent.rs` – the tool loop: ask the model, check, run and audit each call, verify.
  - `prompt.rs` – the system prompt.
  - `tools/` – the `Tool` trait, built-in tools, custom `tool.json` tools and skills.
  - `permissions.rs` – `permissions.json` and the per-call decision.
  - `sandbox/` – the OS sandbox for tools and CLI backends (`macos.rs`, `linux.rs`).
  - `process.rs` – subprocesses with time and output limits.
  - `session.rs`, `context.rs`, `audit.rs`, `eval.rs` – saved chats, summarizing, audit log, evals.
  - `error.rs` – the library's `Error` type.
- `crates/cli` (`uritheking`, binary) – the terminal app.
  - `main.rs` – argument parsing.
  - `repl.rs` – chat session: banner, slash commands, `!` shell.
  - `picker.rs` – arrow-key menus for `/model`.
  - `intro.rs` – startup crown animation.

Adding a backend = one struct implementing `Provider` in `crates/core/src/providers/`,
plus one arm in `build_provider` and one in `catalog::models_for`.
Adding a built-in tool = one struct implementing `Tool` in `crates/core/src/tools/builtin.rs`,
listed in `builtin::all`.

## Behavior

**One-shot prompt.** `harness <provider> -p "text"` prints the reply to stdout and exits 0. On any failure the error goes to stderr and the exit code is 1.

**Chat keeps history.** In chat mode (no `-p`), every request includes all earlier user and assistant turns. A failed turn keeps its completed tool calls and results, records the interruption and saves the session, so a retry can account for changes already made.

**CLI providers get flattened history.** The `claude` and `codex` providers receive multi-message history as a single `User:/Assistant:` transcript prompt.

**Local model auto-detect.** With no model argument, the `local` provider uses the first non-embedding model from `<LOCAL_BASE_URL>/models`. It exits with an error if the server is down or lists no chat model.

**Server error.** When an HTTP provider answers with a non-2xx status, the status code and response body are shown (e.g. bad key, unknown model).

**Open uriTheKing.** Running `uritheking` with no arguments shows a crown banner and opens a chat on the `claude` backend. Each line typed is sent to the model with the full conversation.

**Slash commands and shell.** `/help`, `/model <backend> [model] [effort]`, `/clear`, `/exit` and `!<command>` run locally and are never sent to the model. An unknown `/command` prints a hint to use `/help`; a failed `/model` switch keeps the old backend.

**Pick a model and effort.** `/model` alone opens arrow-key menus for backend, then model, then effort (`default` first). Models come from a built-in list (claude), `~/.codex/models_cache.json` (codex), `<LOCAL_BASE_URL>/models` (local) or OpenRouter's public list. The effort menu lists only that model's levels and is skipped when it has none. Esc keeps the current choice, and a switch keeps the conversation.

**Effort reaches the backend.** A chosen effort (picker or `--effort`) is passed as `--effort <level>` to claude, `-c model_reasoning_effort="<level>"` to codex, `"reasoning": {"effort": ...}` to openrouter and `"reasoning_effort": ...` to local.

**Startup crown animation.** In a real terminal (no `-p`), an ASCII crown hops in from the right three times, squashes on landing and sparkles (~2s), then is erased and the normal banner prints in its place. It is skipped when output is piped or `URITHEKING_NO_INTRO` is set.

## Company tools, skills and permissions

The harness uses four folders, split by who owns the data (the same idea as Claude Code):

| Folder | Where | What's in it |
|---|---|---|
| project | the folder you start `uritheking` in | your code; the only place `read_file` / `write_file` work |
| company | `/Library/Application Support/Uritheking` (macOS), `/etc/uritheking` (Linux), or `URITHEKING_COMPANY` | `COMPANY.md`, `tools/`, `skills/`, `permissions.json`, `evals/`. Installed by IT so users can't edit it. If it's missing, the project folder is used (fine for development, but not locked). |
| data | `~/.uritheking/projects/<project path>/` (`URITHEKING_USER_DIR` replaces `~/.uritheking`) | this user's `sessions/`, `logs/audit.jsonl` and personal `memory/` for this project |
| shared | `~/.uritheking/shared` or `URITHEKING_SHARED` | a clone of the company memory repo: `memory/teams/<role>/`, `memory/company/`. Start it from `shared-memory/` in this repo. |

Files in the company folder:

| Path | What it is |
|---|---|
| `COMPANY.md` | What the company and product are. Sent with every call. |
| `tools/<name>/tool.json` | A company tool: `name`, `description`, `params`, `command`, and optional `reads` (project folders it may read, `"."` = whole project), `writes` (project folders it may write to), `network` (`true` to allow outbound network) and `env` (names of environment variables to pass through). The command runs in the tool's folder with arguments as JSON on stdin and as `ARG_<NAME>` env vars. Whatever it prints is the result. Any language works. |
| `skills/<name>/SKILL.md` | A playbook (frontmatter: `name`, `description`). The model loads it when it fits the task. |
| `permissions.json` | Which tools and skills each role gets, file rules, the approval mode, and the optional `verify` tool. |
| `evals/<name>.json` | Eval cases: a prompt and what should happen. Run with `uritheking eval`. |
| `owners` in `permissions.json` | Roles that may change company memory. |

Memory (`<name>.md` with frontmatter, plus a `MEMORY.md` index in every scope folder; safe to edit by hand, then `/reload`):

| Scope | Folder | Who reads it | Who writes it |
|---|---|---|---|
| personal | `<data>/memory/` | only this user, in this project, when the role has `remember` | the user, via `remember` / `forget` |
| team | `<shared>/memory/teams/<role>/` | everyone in that role | members of that role, then commit and push |
| company | `<shared>/memory/company/` | everyone | owners only, then commit and push |
| `logs/audit.jsonl` | Written by the harness: every tool call, who made it and what was decided. |

Built-in tools: `read_file`, `write_file`, `load_skill`, `remember`, `forget`.

**Remember a fact across sessions.** When a role allowed to use `remember` calls it with `name`, `description` and `content` and the user approves, `memory/<name>.md` is written and `memory/MEMORY.md` lists it as `- <name>: <description>`. The next session (or `/reload`) puts that index in the system prompt inside `<memory>`.

**Save a team memory.** When a `sales` user asks to remember something for the team, `remember` with `scope: "team"` writes to `<shared>/memory/teams/sales/` and the result says to commit and push it. After they pull, every `sales` user sees it inside `<memory scope="team sales">`; other roles don't.

**Only owners write company memory.** With `"owners": ["owner"]` in `permissions.json`, a user whose role isn't listed gets "only owners can change company memory" from `remember` or `forget` with `scope: "company"`, and nothing is written. The owner's calls go to `<shared>/memory/company/`, which every role sees inside `<memory scope="company">`.

**No memory repo cloned yet.** With nothing at `~/.uritheking/shared` and no `URITHEKING_SHARED`, saving a team or company memory fails and says where to clone the company memory repo. Personal memory still works.

**Sessions and memory follow the user, not the repo.** Working in `/work/app`, sessions and personal memory go to `~/.uritheking/projects/-work-app/`, never into `/work/app`. Another project starts with its own sessions and memory.

**Sessions don't fill memory on their own.** Nothing is saved to memory unless `remember` was called and approved. The default scope is `personal`; the model uses `team` or `company` only when the user asks to share.

With the company folder installed where users can't write, users can't change `permissions.json` to get around the owner check. Git protects the shared memory repo too: `shared-memory/CODEOWNERS` lists who must approve each folder (turn on branch protection with "Require review from Code Owners").

**Replace or forget a memory.** Calling `remember` with an existing name (e.g. `db-choice`) overwrites it, and `forget` with `{"name": "db-choice"}` deletes it; either way the index is rebuilt. Forgetting an unknown name returns an error and changes nothing.

**Memory stays safe.** `remember` and `forget` fail without touching anything outside `memory/` when the name is not 1-64 characters of `a-z`, `0-9`, `-` (e.g. `../permissions`) or the `memory/` folder is a symlink. `write_file` can never write `memory/`, both tools count as writes (manual yes after untrusted tool output), and the index is capped at 8 KiB.

**Permissions.** Choose a role with `--role sales` or `URITHEKING_ROLE=sales`:
- `"mode": "ask"` asks y/N before every tool call. `"yolo"` runs calls without asking.
- `files.deny` globs are never touched, even with approval. `files.approve` globs always need a manual yes, even in yolo. Paths outside the project are always denied.
- Built in, whatever the file says: `write_file` can never touch `sessions/`, `logs/` or `memory/`, and writing `permissions.json`, `COMPANY.md`, `tools/` or `skills/` always needs a manual yes (they control what the model may do).
- **Untrusted output.** File contents and tool output can contain instructions planted by someone else. The model is told to treat `<tool_result>` text as data, and tool output can't fake its own end tag or a tool call. Once any tool output is in the conversation, calls that can change things (`write_file`, tools with `writes` or `network`) need a manual yes even in yolo. Set `"trust_tool_output": true` to turn that off.
- If roles are defined, a missing or unknown role gets no tools. Unreadable, malformed, or incorrectly typed permission files fail closed. Unknown keys in `permissions.json`, `tool.json` and eval cases are errors, so a typo like `"verfy"` is reported instead of ignored.
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
