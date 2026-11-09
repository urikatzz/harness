# A "changes" permission mode and a configurable step limit

Date: 2026-11-10
Status: approved design, not yet implemented

## Goal

Make the file tools usable for real tasks. Today a turn stops after 10 model replies, and in
`ask` mode every call needs a y/N, including `list_files`, `search_files` and `read_file`.
Finding, reading and editing a few files runs into one or the other.

## Scope

In:

1. A third permission mode, `"changes"`: ask only before calls that can change things.
2. A `"max_steps"` setting in `permissions.json`, default 50, replacing the fixed limit of 10.

Out:

- A "keep going?" prompt when the limit is reached.
- Per-role modes or per-role limits.
- `run_command`, role identity, central audit, memory sync (separate specs).

## Decisions

- **A new mode, not a change to `ask`.** `ask` and `yolo` behave exactly as before, so no
  existing setup changes. The alternative, making `ask` stop asking for reads, was rejected by
  the owner: it would silently loosen every existing `ask` setup.
- **The rule is the tool's declared effects.** A call runs without a prompt when its tool
  declares no file writes and no network (`!tool.effects().any()`). The alternative, a fixed
  list of built-in read tools, was rejected: a role that mostly uses read-only custom tools
  would gain nothing.
- **One `Mode` enum in `Policy`, replacing `pub yolo: bool`.** The alternative, keeping `yolo`
  and adding a second flag, allows a meaningless combination and makes every check read both.
- **A configurable limit that stops, default 50.** The alternative, asking to continue at the
  limit, needs a new callback and prompt; the history is kept, so the user can type "continue".

## Components

### Permissions (`crates/core/src/permissions.rs`)

- `Mode` becomes public with three values, written in `permissions.json` as `"ask"`,
  `"changes"`, `"yolo"`. `ask` stays the default.
- `Policy` holds `pub mode: Mode` instead of `pub yolo: bool`.
- `Policy::check` is unchanged up to its last line: role lists, skill gate and file rules
  (`deny` refuses, `approve` asks, state folders, harness configuration) apply in every mode.
  The last line becomes:

  | Mode | Tool has no effects | Tool writes files or uses the network |
  |---|---|---|
  | `ask` | ask (`ASK_MODE`) | ask (`ASK_MODE`) |
  | `changes` | allow | ask (`CHANGES_MODE`) |
  | `yolo` | allow | allow |

- New constant `CHANGES_MODE = "this call can change things"`, the reason shown in the prompt.
- `Policy::for_eval` sets `Mode::Ask`, as today, so evals see every call. `eval.rs` approves
  only calls whose reason is `ASK_MODE`, so nothing there changes.
- `max_steps`: an optional number in `PolicyFile`. `0`, negative numbers, fractions and
  non-numbers are config errors. `Policy::max_steps()` returns the configured value or
  `DEFAULT_MAX_STEPS = 50`, including when there is no `permissions.json`.

### Tool loop (`crates/core/src/agent.rs`)

- The `MAX_STEPS` constant is removed; `Turn::run` loops `policy.max_steps()` times.
- What counts as a step is unchanged: every model reply in the turn (tool calls, malformed
  calls, and answers sent back by a failed verify).
- The error at the limit becomes
  `stopped after N steps without a final answer; the work so far is kept, say "continue" to go on`.
- The untrusted-output rule is unchanged. It upgrades `Allow` to `Ask` for calls with effects,
  so it matters only in `yolo`; in `changes` those calls already ask.

### CLI (`crates/cli/src/repl.rs`)

- `/permissions` prints one line per mode:
  `ask — confirm each tool call`, `changes — confirm calls that change files or use the network`,
  `yolo — tools run without asking`.

### This repo's `permissions.json`

Unchanged (`"mode": "ask"`). Switching is the owner's choice.

## Scenarios

#### Scenario: Reads run without a prompt in changes mode

- **GIVEN** `"mode": "changes"` and a role allowed to use `search_files` and `read_file`
- **WHEN** the model calls `search_files` and then `read_file` on a normal file
- **THEN** both run without a y/N
- **AND** both are recorded in the audit log as `allowed`

#### Scenario: Changes still ask in changes mode

- **GIVEN** `"mode": "changes"`
- **WHEN** the model calls `edit_file`, `write_file`, `remember`, or a custom tool that declares `writes` or `network`
- **THEN** the user is asked y/N with the reason `this call can change things`
- **AND** nothing runs if the answer is not yes

#### Scenario: Read-only custom tools run without a prompt

- **GIVEN** `"mode": "changes"` and a custom tool with no `writes` and `network: false`
- **WHEN** the model calls it
- **THEN** it runs without a y/N

#### Scenario: File rules win over the mode

- **GIVEN** `"mode": "changes"`, `.env` in `files.approve` and `secrets/**` in `files.deny`
- **WHEN** the model calls `read_file` on `.env`, and on `secrets/key.txt`
- **THEN** the first asks for a manual yes and the second is denied

#### Scenario: Existing modes are unchanged

- **GIVEN** `"mode": "ask"`, or `"mode": "yolo"`, or no mode at all
- **WHEN** the model calls any tool
- **THEN** `ask` (and no mode) asks for every call and `yolo` asks for none, as before

#### Scenario: Evals still see every call

- **GIVEN** `"mode": "changes"` and an eval case
- **WHEN** the eval runs
- **THEN** read-only calls run and calls that could change things are recorded and declined

#### Scenario: A turn stops at the configured limit

- **GIVEN** `"max_steps": 2` and a model that keeps calling tools
- **WHEN** a turn runs
- **THEN** it stops after 2 model replies with an error naming the limit and saying the work so far is kept
- **AND** the completed calls and results stay in history

#### Scenario: The default limit is 50

- **GIVEN** a `permissions.json` without `max_steps`, or no `permissions.json`
- **WHEN** a turn runs
- **THEN** it may take up to 50 steps

#### Scenario: A bad limit is a config error

- **GIVEN** `"max_steps"` set to `0`, `-1`, `1.5` or `"many"`
- **WHEN** the harness loads `permissions.json`
- **THEN** loading fails with a config error and no tools are available

## Error handling

- An unknown mode (for example `"mode": "change"`) is a config error, as unknown modes are today.
- A bad `max_steps` fails closed like every other malformed permission value.

## Testing

- `permissions.rs`: the three modes against a tool with no effects, `write_file`, and a
  networked custom tool; file rules in `changes` mode; `max_steps` default, value and bad values.
- `agent.rs`: a scripted provider that keeps calling `read_file` stops at `"max_steps": 2`,
  with both results in history.
- Existing tests that build `Policy { yolo: true, .. }` move to `mode: Mode::Yolo`.
- README permissions section and the `permissions.rs` header comment describe the new mode
  and `max_steps`.
