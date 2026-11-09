---
name: standards
description: Coding and documentation standards every dev change must follow
---

- If requirements are unclear, ask specific questions before writing code; don't guess.
- When proposing a solution, give at least two good options with trade-offs
  (performance, complexity, maintainability).
- Code follows OOP and SOLID, is readable without comments, and prefers composition over inheritance.
- Keep business logic separate from infrastructure: no file, DB or network code inside domain logic.
- Follow the existing patterns in the codebase.
- Minimize external calls and redundant reads/writes; handle errors and edge cases explicitly.
- Add short comments only where intent isn't obvious.
- Document significant behavior as scenarios in the README:
  `#### Scenario: <title>` then **GIVEN** / **WHEN** / **THEN** / **AND** bullets.
- Every change keeps `cargo test` green and adds tests for new behavior.

Why: these come from the company AGENTS.md and keep the codebase consistent across people and agents.
