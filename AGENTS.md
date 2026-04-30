# AGENTS.md

Conventions for working on `kobito` itself. Both Claude Code and Codex auto-read this file at every invocation, so an agent driven by kobito (or by any other orchestrator) picks them up without further wiring.

## What kobito is

An autonomous coding agent orchestrator. Spawns `claude` or `codex` CLI in a loop on a working branch, commits each iteration with an LLM-generated message, and persists state under `$XDG_STATE_HOME/kobito/`. Stays deliberately thin: project conventions live in this file, not in kobito's prompts.

## Stack

- Rust, edition 2024.
- Single-binary CLI, distributed via `cargo install`.
- License: **ISC**.

## Source layout

| path | role |
|---|---|
| `src/main.rs` | entry point — `mod` declarations only |
| `src/cli.rs` | clap definitions + dispatch |
| `src/agent/` | `Agent` trait + per-agent modules (`claude_code`, `codex`) and shared `stream` helper |
| `src/runner.rs` | `cont` mode loop + `resume` |
| `src/iteration.rs` | `iter` mode loop |
| `src/state.rs` | XDG state directory layout |
| `src/preset.rs` | preset resolution + `{{var}}` substitution |
| `src/notes.rs` | per-iteration learning generation |
| `src/branch.rs` | agent-driven branch name suggestion |
| `src/commit.rs` | agent-driven commit message generation |
| `src/git.rs` | `git` CLI wrapper |
| `src/logger.rs` | `LogSink` — terminal passthrough + ndjson |
| `src/ui.rs` | indicatif status bar |
| `src/prompt.rs` | iteration / task prompt builders |
| `src/tasks.rs` | `tasks.md` parser |

## Coding style

- Public items at the top of each file.
- Prefer `map` / `filter` / `reduce` over `for` / `while`.
- Single responsibility — keep functions ≤ 15 lines and files ~200 lines or under where reasonable.
- Comments are minimal. Convey behaviour through small, well-named functions. A comment is for the **why** of a non-obvious decision, never to restate the **what**.
- No `unwrap()` / `expect()` in non-test code unless a panic is genuinely the right outcome.
- Use `anyhow::Result` for application errors, `thiserror`-style only when callers need to discriminate.

## Commit messages

Conventional Commits with a scope and a body that explains the **why**:

```
<type>(<scope>): <subject>

<body — usually 2-4 short lines, why this change is being made>
```

- `<type>`: `feat`, `fix`, `refactor`, `perf`, `docs`, `test`, `chore`, `build`, `ci`, `style`, `revert`.
- Imperative subject, no trailing period, ≤ 72 chars.
- No `Co-authored-by` lines unless explicitly requested.
- Reference issues in the body or PR — not via trailers.

## Branch naming

- `feature/<slug>` — new features
- `refactor/<slug>` — non-functional changes
- `fix/<slug>` — bug fixes
- `docs/<slug>` — docs-only changes
- `chore/<slug>` — maintenance, dependency bumps, repo hygiene

`<slug>` is short kebab-case, ideally derived from the goal (e.g. `feature/preset-system`, `refactor/drop-issue-8-features`).

## PR workflow

- One concern per commit; bias toward small commits over squashable mega-commits.
- Every commit must compile (`cargo check`).
- PR body lists each commit with a one-line description and the test plan.
- Merge with `--merge` (regular merge commit). **Never** `--squash`.
- Link issues with `Closes #N` — one keyword per issue. GitHub's parser does not handle `Closes #1, #2, #3`.

## Output language

- Source code, comments, commit messages, PR bodies, this file, `README.md`: **English**.
- Reply language for chat / CLI output is left to the agent's own global memory and is not pinned here.

## Build / test

```sh
cargo build --release       # release binary
cargo test --bin kobito     # unit tests
cargo check                 # quick type check
```

The release profile uses LTO + single codegen unit + strip; expect ~15 s release builds.

## When extending kobito

- Adding an agent backend → one new `src/agent/<name>.rs` module implementing `Agent`, plus a branch in `agent::from_name`. No core loop changes needed.
- Adding state on disk → put it inside `$XDG_STATE_HOME/kobito/projects/<id>/runs/<ts>/` (per-run) unless it genuinely belongs to the project across runs (rare — `tasks.md` is the only example today).
- Don't add orchestrator-side workarounds for things the agent or the project's own AGENTS.md / CLAUDE.md should handle (output language, code style, doc conventions). kobito stays out of those.
