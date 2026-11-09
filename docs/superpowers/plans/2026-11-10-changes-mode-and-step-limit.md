# Changes Mode and Configurable Step Limit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `"changes"` permission mode that asks only before calls that can change things, and a `"max_steps"` setting (default 50) replacing the fixed 10-step limit.

**Architecture:** `Policy` holds one public `Mode` enum instead of the `yolo` flag; the last line of `Policy::check` decides from the mode and the tool's declared effects. The step limit is read from the policy by the tool loop.

**Tech Stack:** Rust 2021, serde.

**Spec:** `docs/superpowers/specs/2026-11-10-changes-mode-and-step-limit-design.md`

---

## Before you start

Baseline on `changes-mode-and-step-limit`: `cargo test --locked --workspace` passes (63 core, 5 CLI) and `cargo clippy --locked --workspace --all-targets -- -D warnings` is clean.

- `Policy::check` (`crates/core/src/permissions.rs`) returns early for the skill gate, role lists and file rules. Only its last line depends on the mode.
- `Tool::effects().any()` is true when a tool writes files or uses the network.
- `eval.rs` approves a call only when the reason equals `ASK_MODE`; `Policy::for_eval` must keep forcing ask mode.
- Match the dense code style; do not run `cargo fmt` over existing files.

## File structure

| File | Change |
|---|---|
| `crates/core/src/permissions.rs` | public `Mode`, `CHANGES_MODE`, `max_steps`, header comment |
| `crates/core/src/agent.rs` | loop limit from the policy, new error text |
| `crates/cli/src/repl.rs` | `/permissions` mode line |
| `README.md` | permissions section |

---

### Task 1: The `Mode` enum and `changes` mode

**Files:**
- Modify: `crates/core/src/permissions.rs`, `crates/cli/src/repl.rs`

- [ ] **Step 1: Write the failing tests**

In `mod tests` of `crates/core/src/permissions.rs`, add after the `Named` tool:

```rust
    /// A custom tool that reaches the network.
    struct Networked;
    impl Tool for Networked {
        fn name(&self) -> &str { "fetch" }
        fn description(&self) -> &str { "" }
        fn params(&self) -> Vec<(&str, &str)> { vec![] }
        fn effects(&self) -> crate::tools::Effects { crate::tools::Effects { writes_files: false, network: true } }
        fn run(&self, _: &Context, _: &Value) -> Result<String> { Ok(String::new()) }
    }

    fn policy_with(permissions: &str) -> (crate::test_support::TestDir, Policy) {
        let dir = crate::test_support::TestDir::new();
        std::fs::write(dir.path().join("permissions.json"), permissions).unwrap();
        let policy = Policy::load(dir.path(), None).unwrap();
        (dir, policy)
    }

    #[test]
    fn changes_mode_asks_only_for_calls_that_can_change_things() {
        let (_dir, p) = policy_with(r#"{"mode":"changes","files":{"deny":["secrets/**"],"approve":[".env"]}}"#);
        let ask = || Decision::Ask(CHANGES_MODE.into());
        assert_eq!(check(&p, "read_file", json!({"path": "a.txt"})), Decision::Allow);
        assert_eq!(check(&p, "list_files", json!({})), Decision::Allow);
        assert_eq!(check(&p, "search_files", json!({"pattern": "x"})), Decision::Allow);
        // A custom tool that declares no writes and no network.
        assert_eq!(check(&p, "company_info", json!({})), Decision::Allow);
        for tool in ["write_file", "edit_file"] {
            assert_eq!(check(&p, tool, json!({"path": "a.txt"})), ask(), "{tool}");
        }
        assert_eq!(check(&p, "remember", json!({})), ask());
        assert_eq!(p.check(&Networked, &json!({})), ask());
        // File rules win over the mode.
        assert!(matches!(check(&p, "read_file", json!({"path": ".env"})), Decision::Ask(why) if why != CHANGES_MODE));
        assert!(matches!(check(&p, "read_file", json!({"path": "secrets/key.txt"})), Decision::Deny(_)));
        // Evals still see every call.
        assert_eq!(check(&p.for_eval(), "read_file", json!({"path": "a.txt"})), Decision::Ask(ASK_MODE.into()));
    }

    #[test]
    fn ask_and_yolo_modes_are_unchanged() {
        for permissions in [r#"{"mode":"ask"}"#, "{}"] {
            let (_dir, p) = policy_with(permissions);
            assert_eq!(check(&p, "read_file", json!({"path": "a.txt"})), Decision::Ask(ASK_MODE.into()));
            assert_eq!(check(&p, "write_file", json!({"path": "a.txt"})), Decision::Ask(ASK_MODE.into()));
        }
        let (_dir, p) = policy_with(r#"{"mode":"yolo"}"#);
        assert_eq!(check(&p, "read_file", json!({"path": "a.txt"})), Decision::Allow);
        assert_eq!(check(&p, "write_file", json!({"path": "a.txt"})), Decision::Allow);
        assert_eq!(p.check(&Networked, &json!({})), Decision::Allow);
    }
```

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test -p uritheking-core permissions`
Expected: compile error, `cannot find value CHANGES_MODE`.

- [ ] **Step 3: Implement**

In `crates/core/src/permissions.rs`:

Add under `ASK_MODE`:

```rust
/// The reason given in "changes" mode for a call that writes files or uses the network.
pub const CHANGES_MODE: &str = "this call can change things";
```

Replace the `Mode` enum with:

```rust
/// When a call that passed every other rule needs a human yes.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Before every call.
    #[default]
    Ask,
    /// Only before calls that write files or use the network.
    Changes,
    /// Never.
    Yolo,
}
```

In `struct Policy`, replace `pub yolo: bool,` with `pub mode: Mode,`.

In `load_with`, replace `yolo: file.mode == Mode::Yolo,` with `mode: file.mode,`.

In `for_eval`, replace `Policy { yolo: false, ..self.clone() }` with `Policy { mode: Mode::Ask, ..self.clone() }`.

In `check`, replace the last line (`if self.yolo { ... }`) with:

```rust
        match (self.mode, tool.effects().any()) {
            (Mode::Yolo, _) | (Mode::Changes, false) => Decision::Allow,
            (Mode::Changes, true) => Decision::Ask(CHANGES_MODE.into()),
            (Mode::Ask, _) => Decision::Ask(ASK_MODE.into()),
        }
```

In the existing tests `roles_and_files` and `harness_files_are_guarded_even_in_yolo`, replace `yolo: true,` with `mode: Mode::Yolo,`.

In `crates/cli/src/repl.rs`, replace the line that prints the mode (`if p.yolo { "yolo — tools run without asking" } else { "ask — confirm each tool call" }`) so the statement reads:

```rust
        let mode = match p.mode {
            uritheking_core::permissions::Mode::Ask => "ask — confirm each tool call",
            uritheking_core::permissions::Mode::Changes => "changes — confirm calls that change files or use the network",
            uritheking_core::permissions::Mode::Yolo => "yolo — tools run without asking",
        };
        println!("  {BOLD}mode{RESET}  {mode}");
```

- [ ] **Step 4: Run the tests**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: all pass (65 core tests), no warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/permissions.rs crates/cli/src/repl.rs
git commit -m "Add changes mode: ask only before calls that can change things"
```

---

### Task 2: Configurable step limit

**Files:**
- Modify: `crates/core/src/permissions.rs`, `crates/core/src/agent.rs`

- [ ] **Step 1: Write the failing tests**

In `mod tests` of `crates/core/src/permissions.rs`:

```rust
    #[test]
    fn max_steps_defaults_to_50_and_rejects_bad_values() {
        assert_eq!(Policy::default().max_steps(), 50);
        assert_eq!(policy_with("{}").1.max_steps(), 50);
        assert_eq!(policy_with(r#"{"max_steps":2}"#).1.max_steps(), 2);
        let dir = crate::test_support::TestDir::new();
        for bad in [r#"{"max_steps":0}"#, r#"{"max_steps":-1}"#, r#"{"max_steps":1.5}"#, r#"{"max_steps":"many"}"#] {
            std::fs::write(dir.path().join("permissions.json"), bad).unwrap();
            assert!(Policy::load(dir.path(), None).is_err(), "{bad}");
        }
    }
```

In `mod tests` of `crates/core/src/agent.rs`, after `cap_cuts_at_a_character_boundary`:

```rust
    #[test]
    fn a_turn_stops_at_the_configured_step_limit_and_keeps_its_work() {
        let dir = project(r#"{"mode":"yolo","max_steps":2}"#);
        let (tb, policy) = (Toolbox::load(dir.path()), Policy::load(dir.path(), None).unwrap());
        let call = r#"<tool_call>{"name":"read_file","args":{"path":"notes.txt"}}</tool_call>"#;
        let provider = script(&[call, call, call]);
        let mut history = vec![Message::user("read forever")];
        let err = run_turn(&provider, &tb, &policy, &mut history, |_| {}, |_, _, _| true).unwrap_err().to_string();
        assert_eq!(err, "stopped after 2 steps without a final answer; the work so far is kept, say \"continue\" to go on");
        // The user message, then two calls with their results.
        assert_eq!(history.len(), 5);
        assert_eq!(history.iter().filter(|m| m.content.starts_with("<tool_result name=\"read_file\">")).count(), 2);
    }
```

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test -p uritheking-core max_steps`
Expected: compile error, `no method named max_steps`.

- [ ] **Step 3: Implement**

In `crates/core/src/permissions.rs`:

Add under `CHANGES_MODE`:

```rust
/// Model replies allowed in one turn when `max_steps` is not set.
const DEFAULT_MAX_STEPS: usize = 50;
```

Add to `struct PolicyFile`, after `trust_tool_output`:

```rust
    /// Model replies allowed in one turn.
    max_steps: Option<u64>,
```

Add to `struct Policy`, after `trust_tool_output`:

```rust
    max_steps: Option<usize>,
```

In `load_with`, after the `verify` check, add:

```rust
        if file.max_steps == Some(0) {
            return Err(Error::config("max_steps must be 1 or more"));
        }
```

and in the `Policy { ... }` it builds, add `max_steps: file.max_steps.map(|n| n as usize),` after `trust_tool_output`.

Add to `impl Policy`, after `for_eval`:

```rust
    /// Safety stop so a confused model can't loop forever: model replies allowed in one turn.
    pub fn max_steps(&self) -> usize {
        self.max_steps.unwrap_or(DEFAULT_MAX_STEPS)
    }
```

In `crates/core/src/agent.rs`:

Delete these two lines:

```rust
/// Safety stop so a confused model can't loop forever.
const MAX_STEPS: usize = 10;
```

In `Turn::run`, replace `for _ in 0..MAX_STEPS {` with:

```rust
        let max_steps = self.policy.max_steps();
        for _ in 0..max_steps {
```

and replace the final `Err(...)` of `run` with:

```rust
        Err(format!("stopped after {max_steps} steps without a final answer; the work so far is kept, say \"continue\" to go on").into())
```

- [ ] **Step 4: Run the tests**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: all pass (67 core tests), no warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/permissions.rs crates/core/src/agent.rs
git commit -m "Make the step limit configurable with max_steps, default 50"
```

---

### Task 3: Documentation

**Files:**
- Modify: `crates/core/src/permissions.rs` (header comment), `README.md`

- [ ] **Step 1: Header comment**

In `crates/core/src/permissions.rs`, replace the line

```rust
//!   "mode": "ask",                       // "ask": confirm every tool call · "yolo": run without asking
```

with

```rust
//!   "mode": "ask",                       // "ask": confirm every tool call · "changes": confirm only calls
//!                                        // that write files or use the network · "yolo": run without asking
```

and replace the line starting `//!   "trust_tool_output": false` with these two (keep the existing comment text on the first):

```rust
//!   "trust_tool_output": false,           // true: yolo stays yolo after reading tool output
//!   "max_steps": 50                       // model replies allowed in one turn (default 50)
```

- [ ] **Step 2: README**

In `README.md`, replace

```
- `"mode": "ask"` asks y/N before every tool call. `"yolo"` runs calls without asking.
```

with

```
- `"mode": "ask"` asks y/N before every tool call. `"changes"` asks only before calls that can change things: `write_file`, `edit_file`, `remember`, `forget` and custom tools with `writes` or `network`. Reads, searches and read-only custom tools run without asking. `"yolo"` runs calls without asking.
- `"max_steps"` is how many model replies one turn may take (default 50). At the limit the turn stops with an error; the work so far is kept, so you can say "continue".
```

- [ ] **Step 3: Verify and commit**

Run: `cargo test --locked --workspace && cargo clippy --locked --workspace --all-targets -- -D warnings`
Expected: all pass, no warnings.

```bash
git add crates/core/src/permissions.rs README.md
git commit -m "Document changes mode and max_steps"
```

---

## Scenario coverage

| Spec scenario | Test |
|---|---|
| Reads run without a prompt; changes still ask; read-only custom tools; file rules win; evals see every call | `changes_mode_asks_only_for_calls_that_can_change_things` |
| Existing modes are unchanged | `ask_and_yolo_modes_are_unchanged` |
| A turn stops at the configured limit | `a_turn_stops_at_the_configured_step_limit_and_keeps_its_work` |
| The default limit is 50; a bad limit is a config error | `max_steps_defaults_to_50_and_rejects_bad_values` |
