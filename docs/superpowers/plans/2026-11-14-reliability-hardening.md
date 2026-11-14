# Reliability Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Retry transient model errors with backoff, save the session after every tool step, and record model latency and token usage in the audit log.

**Architecture:** `Error::Http` carries the status and `Retry-After`; `Error::is_transient` and the new `retry::Backoff` decide what to retry and how long to wait, with no I/O. `Turn::ask_model` owns the retry loop and the `model` audit line; its sleep and jitter are injected so tests don't wait. A new `Event::Checkpoint` hands the history to the REPL after each tool step.

**Tech Stack:** Rust 2021, ureq 2, serde_json. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-11-14-reliability-hardening-design.md`

---

## Before you start

Baseline on `reliability-hardening`: `cargo test --locked --workspace` passes (70 core, 5 CLI). `cargo clippy --locked --workspace --all-targets -- -D warnings` already fails on one existing lint (`items_after_test_module` in `providers/cli.rs`); add no new ones.

- `run_turn` keeps its signature (callers: `repl.rs`, `main.rs`, `eval.rs`); tests reach the loop through a crate-private `run_turn_with` that takes the waits.
- Real waits go through `interrupt::cancellable`, so Ctrl-C ends the turn with `Error::Cancelled`.
- Match the dense code style; do not run `cargo fmt` over existing files.

## File structure

| File | Change |
|---|---|
| `crates/core/src/error.rs` | `Http { status, retry_after, message }`, `is_transient`, `From<ureq::Error>` |
| `crates/core/src/retry.rs` | new: `Backoff`, `retry_after_secs` |
| `crates/core/src/provider.rs` | `Usage`, `Reply::usage` |
| `crates/core/src/providers/openai.rs` | status, `Retry-After` and `usage` from the response |
| `crates/core/src/agent.rs` | retry loop, `Event::Retry`, `Event::Checkpoint`, `model` line, `ms` on results |
| `crates/core/src/test_support.rs` | `Scripted` provider that can return errors and usage |
| `crates/cli/src/repl.rs`, `crates/cli/src/main.rs` | retry line; REPL saves on checkpoint |
| `README.md` | Retries, Audit log and session paragraphs |

---

### Task 1: Structured HTTP errors

**Files:** `error.rs`, `retry.rs` (`retry_after_secs` only), `lib.rs`, `providers/openai.rs`

- [ ] Tests in `error.rs`: `is_transient` is true for transport errors (`status: None`) and 408, 429, 500, 502, 503, 504; false for 400, 401, 404, 501, `Cancelled`, `Io`, `Json`, `Other`. `Display` prints only the message.
- [ ] Test in `retry.rs`: `retry_after_secs` reads `"7"` and `" 7 "`, ignores an HTTP date and garbage.
- [ ] Implement. `From<ureq::Error>`: a status keeps its code and `Retry-After`; transport errors that mean the server could not be reached (DNS, connect, I/O, proxy connect) get `status: None`; other transport errors (bad URL, bad header, too many redirects) are `Error::Other`, so they are not retried. `openai.rs` keeps the response body in the message.
- [ ] Commit: "Give HTTP errors a status and Retry-After"

### Task 2: Backoff schedule

**Files:** `retry.rs`

- [ ] Tests: with zero jitter the waits are 1s, 2s, 4s, then `None`; with maximum jitter each wait is under 1.25x its base; `Retry-After` replaces the wait and is capped at 30s.
- [ ] Implement `Backoff::new(jitter)` and `Backoff::next(retry_after) -> Option<Duration>`; the jitter source returns a fraction in `[0, 1)`.
- [ ] Commit: "Add a backoff schedule for model retries"

### Task 3: Token usage in replies

**Files:** `provider.rs`, `providers/openai.rs`

- [ ] Tests in `openai.rs`: `usage` with `prompt_tokens` and `completion_tokens` parses; a body without it gives `None`.
- [ ] Implement `Usage`, `Reply::usage`, and fix every `Reply { .. }`.
- [ ] Commit: "Read token usage from OpenAI-compatible replies"

### Task 4: Retry loop

**Files:** `agent.rs`, `test_support.rs`, `repl.rs`, `main.rs`

- [ ] Tests in `agent.rs` with `test_support::Scripted` and a no-op sleep: transient error then success returns the reply with one `Event::Retry`; a non-transient error fails after one attempt; four transient errors fail with the last error.
- [ ] Implement `Event::Retry { attempt, wait, error }`, the loop in `ask_model`, `run_turn_with`, the real cancellable sleep and a clock-derived jitter. REPL and `-p` print `retrying in 2s (HTTP 429: …)` dimmed.
- [ ] Commit: "Retry transient model errors with backoff"

### Task 5: Model telemetry

**Files:** `agent.rs`

- [ ] Tests: a `model` line with `ms`, `attempts: 2`, `ok` and usage; one without usage; a failed call has `ok: false` and a cut `error`; tool `result` lines have `ms`; an unwritable log does not stop a plain answer.
- [ ] Implement timing in `ask_model` and `execute`, writing through the audit helper (the `model` line has no `tool` field).
- [ ] Commit: "Log model latency, attempts and token usage"

### Task 6: Checkpoints

**Files:** `agent.rs`, `repl.rs`

- [ ] Tests: one `Event::Checkpoint` per tool step (run, denied, invalid call, failed verify), each ending with that step's result; in `repl.rs`, the session file holds the tool result while the turn is still running.
- [ ] Implement the event at the end of `call_tool`, `reject_bad_call` and the verify-failure branch of `accept`; the REPL saves on it, `main.rs` ignores it.
- [ ] Commit: "Save the session after every tool step"

### Task 7: Documentation

**Files:** `README.md`, `audit.rs` header

- [ ] Retries paragraph under Behavior; Audit log paragraph mentions `model` lines and `ms`; Chat keeps history says the session is saved after each tool step.
- [ ] Run `cargo test --locked --workspace` and `cargo clippy --locked --workspace --all-targets -- -D warnings`.
- [ ] Commit: "Document retries, checkpoints and model audit lines"
