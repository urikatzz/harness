# File tools, output caps and CI

Date: 2026-11-09
Status: approved design, not yet implemented

## Goal

Make the harness useful for real work in a project, and protect it with CI before more is built on it.

Today the model has `read_file` and `write_file` only. It cannot find files, search them, or change
part of a file, and one large read can overflow the context window. Nothing runs the tests
automatically, and a release can be published from a broken build.

## Scope

In:

1. Three new built-in tools: `list_files`, `search_files`, `edit_file`.
2. Paging for `read_file`.
3. A size cap on every tool result before it enters history.
4. A CI workflow, and a test gate in the release workflow.

Out (each gets its own spec):

- `run_command` (sandboxed command execution).
- The 10-step limit and the approval prompt on every read in ask mode.
- Role identity, central audit, memory sync.

## Decisions

- **Walking and matching use the `ignore` and `regex` crates.** `ignore` honors `.gitignore`, so
  build output and dependency folders are skipped in any kind of project. The alternative, a
  hand-written skip list and substring search with no new dependencies, was rejected: the list
  would be wrong outside Rust projects.
- **Protected files are skipped silently by list and search.** This matches the rule custom tools
  already follow for `deny` and `approve` globs. The alternative, asking for approval per file,
  would put a prompt in the middle of a search.
- **Both per-tool limits and a global cap.** Per-tool limits give the model a useful note on how
  to continue. The global cap is the safety net for custom tools.

## Components

### Tools (`crates/core/src/tools/`)

`builtin.rs` is 60 lines today and would triple. The new tools go in a new `tools/files.rs`
(`ListFiles`, `SearchFiles`, `EditFile`), with `ReadFile` and `WriteFile` moved there too, so
`builtin.rs` keeps `all()` and `LoadSkill`. All are listed in `builtin::all`.

| Tool | Arguments | Result | Effects |
|---|---|---|---|
| `list_files` | `path` (folder, optional, default project root), `glob` (optional) | project-relative paths, one per line, sorted | none |
| `search_files` | `pattern` (regex), `path` (optional), `glob` (optional) | `path:line: text` per match | none |
| `edit_file` | `path`, `old`, `new`, `replace_all` (optional, default false) | `replaced N occurrence(s)` | writes files |
| `read_file` | `path`, `offset` (first line, optional, default 1), `limit` (lines, optional, default 2000) | the lines, plus a note when more remain | none |

Limits:

- `list_files`: 500 entries. Over that, the result ends with how many were left out and a hint to
  pass `path` or `glob`.
- `search_files`: 200 matches, each line cut to 300 characters. Files over 1 MiB and files that are
  not valid UTF-8 are skipped.
- `read_file`: 2,000 lines by default. When lines remain, the result ends with
  `[lines A-B of N; pass offset to read more]`. The 1 MiB file limit stays.
- `edit_file`: the file must be within the 1 MiB limit. `old` must be non-empty and differ from `new`.

### Project walk (`crates/core/src/tools/walk.rs`)

One unit used by both `list_files` and `search_files`:

```rust
/// Project-relative paths of regular files under `start` that the role may read without approval.
pub(crate) fn readable_files(root: &Path, start: &str, glob: Option<&str>, policy: &Policy) -> Result<Vec<String>>
```

- Built on `ignore::WalkBuilder` with `follow_links(false)`, hidden files included, `.git/` excluded.
- `start` is checked with `paths::resolve` first, so traversal and symlinked folders are rejected.
- Every candidate goes through a new `Policy::readable_without_approval(path) -> bool`. Paths
  matching `files.deny` or `files.approve` are dropped. Symlinks are dropped.
- `glob` uses the same matcher as `permissions.rs`, so there is one glob dialect.
- File contents are always opened through `read_project_file` (which uses `paths::open`), so the
  symlink and hard-link protections stay in one place.

### Permissions (`crates/core/src/permissions.rs`)

- `Policy::readable_without_approval` is a thin public wrapper over the existing `check_path`
  with `Access::Read`: true only when `check_path` returns `None`.
- `list_files` and `search_files` declare `path_arg` as `("path", Access::Read)`, so an explicit
  `path` that is denied is refused up front like any other read.
- `edit_file` declares `path_arg` as `("path", Access::Write)`. It inherits every existing write
  rule: state folders denied, harness configuration and `approve` globs need a manual yes.
- `permissions.json` in this repo: `art` gains `list_files`, `search_files`, `edit_file`. `sales`
  is unchanged. Roles with `"*"` get the tools automatically.

### Output cap (`crates/core/src/agent.rs`)

- New constant `MAX_RESULT_BYTES = 64 * 1024`.
- `tool_result` cuts longer output at a character boundary and appends
  `[output cut: N of M bytes shown]`. The cut happens before `neutralize`, and applies to built-in
  tools, custom tools and verify output alike.
- The audit `result` line keeps recording the full byte count.

### CI (`.github/workflows/`)

- New `ci.yml`, on push and pull request, matrix of `macos-latest` and `ubuntu-22.04`:
  `cargo test --locked --workspace` and `cargo clippy --locked --workspace --all-targets -- -D warnings`.
  The Ubuntu job installs `bubblewrap` so sandbox tests run.
- `ci.yml` also declares `workflow_call`. `release.yml` gains a `test` job that calls it, and
  `build` gets `needs: test`.
- Any clippy warnings in the current code are fixed as the first step of the implementation.

## Scenarios

#### Scenario: List files in a project

- **GIVEN** a project with `src/main.rs`, `README.md` and a git-ignored `target/` folder
- **WHEN** the model calls `list_files` with no arguments
- **THEN** the result lists `README.md` and `src/main.rs`
- **AND** nothing under `target/` or `.git/` is listed

#### Scenario: Protected files are hidden from listing and search

- **GIVEN** `permissions.json` with `secrets/**` in `files.deny` and `.env` in `files.approve`
- **WHEN** the model calls `list_files`, or `search_files` with a pattern that occurs in `.env`
- **THEN** neither `.env` nor anything under `secrets/` appears in the result
- **AND** no approval prompt is shown for them

#### Scenario: Symlinks are not followed

- **GIVEN** a project containing a symlink to a file outside the project
- **WHEN** the model calls `list_files` or `search_files`
- **THEN** the symlink is not listed and its target is not read

#### Scenario: Search returns matching lines

- **GIVEN** `src/agent.rs` contains `const MAX_STEPS` on line 16
- **WHEN** the model calls `search_files` with pattern `MAX_STEPS` and glob `**/*.rs`
- **THEN** the result contains `src/agent.rs:16:` followed by that line

#### Scenario: Too many search matches

- **GIVEN** a pattern that matches more than 200 lines
- **WHEN** the model calls `search_files`
- **THEN** the first 200 matches are returned
- **AND** the result ends with a note that more exist and to narrow the pattern, `path` or `glob`

#### Scenario: Invalid search pattern

- **GIVEN** any project
- **WHEN** the model calls `search_files` with a pattern that is not a valid regex
- **THEN** the call fails with the regex error and no file is read

#### Scenario: Edit one place in a file

- **GIVEN** `notes.txt` contains `hello world` once
- **WHEN** the model calls `edit_file` with `old: "world"`, `new: "team"` and the user approves
- **THEN** the file contains `hello team` and the result is `replaced 1 occurrence(s)`
- **AND** the call counts as a file change, so the `verify` tool runs before the answer is accepted

#### Scenario: Ambiguous or missing edit target changes nothing

- **GIVEN** `notes.txt` contains `foo` twice, and does not contain `bar`
- **WHEN** the model calls `edit_file` with `old: "foo"` without `replace_all`, or with `old: "bar"`
- **THEN** the call fails, saying how many times `old` was found
- **AND** the file is unchanged

#### Scenario: Replace every occurrence

- **GIVEN** `notes.txt` contains `foo` twice
- **WHEN** the model calls `edit_file` with `old: "foo"`, `new: "baz"`, `replace_all: true`
- **THEN** both are replaced and the result is `replaced 2 occurrence(s)`

#### Scenario: Edit follows the write rules

- **GIVEN** a role in yolo mode
- **WHEN** the model calls `edit_file` on `permissions.json`, and on `logs/audit.jsonl`
- **THEN** the first needs a manual yes and the second is denied
- **AND** an edit after any untrusted tool output needs a manual yes, as `write_file` does

#### Scenario: Read a long file in pages

- **GIVEN** a file of 5,000 lines
- **WHEN** the model calls `read_file` with only `path`
- **THEN** lines 1-2000 are returned, ending with `[lines 1-2000 of 5000; pass offset to read more]`
- **AND** a second call with `offset: 2001` returns the next 2,000 lines

#### Scenario: Oversized tool output is cut

- **GIVEN** a custom tool that prints 500 KiB
- **WHEN** the model calls it
- **THEN** the result in history holds the first 64 KiB and ends with `[output cut: 65536 of 512000 bytes shown]`
- **AND** the audit log records the full 512000 bytes

#### Scenario: CI blocks a broken change

- **GIVEN** a pull request with a failing test or a clippy warning
- **WHEN** CI runs
- **THEN** the workflow fails on both macOS and Ubuntu

#### Scenario: A release is not published from a failing build

- **GIVEN** a `v*` tag pushed on a commit whose tests fail
- **WHEN** the release workflow runs
- **THEN** the `test` job fails and no binaries are built or attached to a release

## Error handling

- Missing or wrongly typed arguments fail with a message naming the argument, as the existing tools do.
- A `path` that is not a folder (list, search) or not a regular file (edit, read) fails without reading anything.
- Unreadable files met during a search are skipped; the search continues.
- `edit_file` reads, checks and only then writes. A failed check never opens the file for writing.

## Testing

- Unit tests for each scenario above, in the module that owns the behavior, using `TestDir`.
- One tool-loop test in `agent.rs` for the output cap with a scripted provider.
- New eval `evals/find_definition.json`: role `dev`, a "where is X defined" prompt, expects
  `search_files` and no `write_file`.
- README: `Built-in tools` list and the permissions section updated in prose.
