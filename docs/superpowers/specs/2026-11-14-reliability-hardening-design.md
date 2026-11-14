# Reliability hardening: retry, checkpoints, model telemetry

Date: 2026-11-14

## Goal

Make a turn survive the ordinary failures of a model backend and leave a record of what each
model call cost. Three changes, no change to the tool-call protocol:

1. Retry transient model errors with backoff.
2. Save the session after every completed tool step.
3. Record latency and token usage in the audit log.

## 1. Retry with backoff

### Behavior

- A model call that fails with a **transient** error is retried up to 3 times (4 attempts in total).
- Transient means: the server could not be reached or the connection broke, or it answered
  HTTP 408, 429, 500, 502, 503 or 504.
- Every other error fails at once, as today: other 4xx statuses (bad key, unknown model), a
  malformed response body, `Error::Cancelled`, and every error from the `claude` and `codex`
  CLI backends.
- Waits are 1s, 2s and 4s, each plus up to 25% random jitter.
- If the server sent `Retry-After` in seconds, that value replaces the wait, capped at 30s.
  A `Retry-After` in HTTP-date form is ignored.
- Ctrl-C during a wait ends the turn with `Error::Cancelled`.
- When the retries run out, the turn fails with the last error.
- The compaction request in `context.rs` is not retried; its failure is already non-fatal.

### Design

- `error.rs`: `Error::Http` becomes
  `Http { status: Option<u16>, retry_after: Option<u64>, message: String }`.
  `status` is `None` for transport errors. `Error::is_transient()` implements the rule above.
  `Display` still prints only the message.
- `providers/openai.rs`: fills `status` and `retry_after` from the response.
- `retry.rs` (new, no I/O): `Backoff` yields the wait for each retry, given an optional
  `Retry-After` and a jitter source. It knows nothing about providers or sleeping.
- `agent.rs`: `Turn::ask_model` owns the retry loop. Before each wait it emits
  `Event::Retry { attempt, wait, error }`. The wait goes through `interrupt::cancellable` so
  Ctrl-C works. The sleep function is a field on `Turn` so tests don't sleep.
- `repl.rs` and `main.rs`: print one dim line per `Event::Retry`, e.g.
  `retrying in 2s (HTTP 429: rate limited)`.

## 2. Checkpoint after each step

### Behavior

- After each tool result (run, denied, or invalid call) is appended to the history, the chat
  session is saved.
- If the process is killed mid-turn, `umango -c` reopens a session that ends with the last
  tool result. The user says "continue".
- A save that fails prints the existing "couldn't save session" message and the turn goes on.
- One-shot `-p` runs and evals do not save sessions, as today.

### Design

- `agent.rs`: new `Event::Checkpoint { history: &[Message] }`, emitted at the end of
  `call_tool`, `reject_bad_call` and the verify-failure branch of `accept`.
- `repl.rs`: the event handler calls `Session::save`, which already writes to a temp file and
  renames it.
- The whole session file is rewritten at each step. History is bounded by the context budget,
  so this is at most a few hundred KiB per step.

## 3. Model telemetry in the audit log

### Behavior

- Every model call in the tool loop writes one `model` line to `logs/audit.jsonl` when it
  finishes, whether it succeeded or failed:

  ```json
  {"event":"model","role":"dev","ms":1840,"attempts":1,"ok":true,"input_tokens":5120,"output_tokens":212,"ts":1794690000000}
  ```

- `ms` covers all attempts and waits. `attempts` counts requests sent.
- `input_tokens` and `output_tokens` are present only when the backend reported them
  (`openrouter`, and `local` servers that send `usage`). The CLI backends never report them.
- A failed call adds `"error"` with the message cut to 200 characters.
- Tool `result` lines gain `ms`, the time the tool ran.
- If a `model` line can't be written, `Event::AuditFailed` fires and the turn continues. Only
  tool decisions fail closed.

### Design

- `provider.rs`: `Reply` gains `usage: Option<Usage>`;
  `Usage { input_tokens: u64, output_tokens: u64 }`.
- `providers/openai.rs`: reads `usage.prompt_tokens` and `usage.completion_tokens`.
- `agent.rs`: `ask_model` times the call and writes the line through the existing `audit` helper;
  `execute` times the tool call.

## Out of scope

- Cost in currency, a `/usage` command, and backend or model names in the audit line.
- Stopping the abandoned request thread after Ctrl-C.
- Token counts for CLI backends.
- Retrying compaction or model-list requests.

## Tests

All with the scripted provider in `test_support`, and an injected no-op sleep:

- `retry.rs`: the wait sequence, the jitter bound, `Retry-After` replacing and being capped.
- `error.rs`: `is_transient` for each status class and for transport errors.
- `agent.rs`:
  - transient error then success returns the reply, with one `Event::Retry` and `attempts: 2` logged;
  - a non-transient error fails after one attempt;
  - four transient errors fail the turn with the last error;
  - one `Event::Checkpoint` per tool step, each carrying the tool result;
  - a `model` line with usage, one without, and `ms` on tool `result` lines.
- `openai.rs`: usage parsed from a response body; absent usage gives `None`.
- `repl.rs`: a checkpoint event writes the session file before the turn ends.

## Documentation

`README.md` gets a "Retries" paragraph under Behavior, and the Audit log and session
paragraphs are updated.
