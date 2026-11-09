---
name: architecture
description: How uriTheKing is built: crates, layers and where new code goes
---

Cargo workspace, two crates:
- `crates/core` (library): models, tools, permissions, sandbox, memory, sessions. No terminal code.
- `crates/cli` (binary): argument parsing, REPL, pickers, intro animation.

Key pieces in core:
- `Provider` trait (`provider.rs`); backends in `providers/` (OpenAI-compatible HTTP, claude/codex CLI).
- `agent.rs`: the tool loop: ask the model, check permissions, run, audit, verify.
- `tools/`: one `Tool` trait for built-in tools, custom `tool.json` tools, skills and memory.
- `permissions.rs`: `permissions.json` roles, file rules and per-call decisions.
- `sandbox/`: OS sandbox for tools and CLI backends (macOS sandbox-exec, Linux bubblewrap).
- `layout.rs`: four folders: project (file tools), company (locked config), data
  (`~/.uritheking/projects/<project>/`: sessions, logs, personal memory), shared (memory repo).

Where new code goes:
- New backend: a struct implementing `Provider`, plus one arm in `build_provider` and `catalog::models_for`.
- New built-in tool: a struct implementing `Tool` in `tools/builtin.rs`, listed in `builtin::all`.

Why: keeps terminal code, domain logic and file/network access apart, so each can change alone.
