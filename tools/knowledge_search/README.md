# Umango knowledge search

`knowledge_search` is a custom Umango tool. It runs a local `rag-cli` binary and returns
ranked passages as JSON. Umango's model generates the answer and cites the returned document
and chunk IDs. Search does not call a model, access the network, or write an evaluation run.
The wrapper creates temporary symbolic links to the immutable index files in Umango's per-call
scratch folder, where Tantivy can create its reader locks. It cleans up its temporary folder
on exit; it does not copy the corpus or modify the installed index. Replace an installed snapshot
only while no searches are running.

The current index contains synthetic EnterpriseRAG-Bench documents, not live company data.
The tool description makes that distinction visible to the model.

## Install

Build with `cargo build --release -p rag-cli`. In your Umango company directory, create
`tools/knowledge_search/` containing:

```text
tool.json           # from knowledge_search/tool.json
run.sh              # from knowledge_search/run.sh
.gitignore          # excludes generated binary, config and index from commits
rag-cli             # copy target/release/rag-cli, preserving executable permissions
experiment.toml     # copy experiments/keyword-structure.toml unchanged
indexes/
  keyword-structure-db17d0b7/  # copy the complete existing index build
```

Paths in `experiment.toml` are relative to the tool directory. Its `indexes_dir = "indexes"`
already matches this layout. Search needs only the index and config; the raw dataset and
questions are not required. Preserve dataset/subset/chunker settings because they determine
the build ID. Install a separate binary compiled for each operating system and architecture.
Refresh the bundled index when the corpus changes; it is a snapshot.

Add `knowledge_search` to the intended role's `tools` list in `permissions.json` if that
role does not already have `"*"`. The repository's default `dev` and `owner` roles already
allow it; `sales` and `art` do not. Reload an open Umango session with `/reload`, then check
`/tools`. Use `UMANGO_COMPANY` to select the company directory when running elsewhere.
Existing approval and audit behavior still applies.

Example model tool call:

```json
{"name":"knowledge_search","args":{"query":"cached token billing","k":8}}
```

For a direct smoke check from the installed tool directory:

```sh
ARG_QUERY='cached token billing' ARG_K=2 sh run.sh
```

The tool has no `reads`, `writes`, `network`, or `env` grants. Its bundled files are readable
under Umango's existing tool-directory sandbox grant. Granting this tool grants access to
the **entire bundled corpus**: there is currently no document-level permission filtering.
Bundle only documents every authorized role may read, or use separate role-specific indexes
and tools. Do not use a shared mixed-permission company corpus until filtering is implemented.

#### Scenario: Retrieve evidence for an answer

- **GIVEN** an installed index and a role allowed to use `knowledge_search`
- **WHEN** Umango executes the approved tool call with a query and optional `k`
- **THEN** the tool returns at most `k` ranked passages with scores and citation identifiers
- **AND** the model can cite `chunk_id` (for example `dsid_fx01#0`) and the document title

#### Scenario: No evidence or invalid request

- **GIVEN** an installed index
- **WHEN** a valid query matches nothing
- **THEN** the tool returns a successful JSON response with an empty `results` array
- **AND** blank or nonsearchable queries, invalid `k`, and missing indexes fail with an error

#### Scenario: Role has no search access

- **GIVEN** the role's tool list excludes `knowledge_search`
- **WHEN** Umango checks the requested tool call
- **THEN** Umango denies it using its existing permissions system
- **AND** search does not add or change role permissions
