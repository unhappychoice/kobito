# kobito

[![CI](https://github.com/unhappychoice/kobito/actions/workflows/ci.yml/badge.svg)](https://github.com/unhappychoice/kobito/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/kobito.svg)](https://crates.io/crates/kobito)
[![Downloads](https://img.shields.io/crates/d/kobito.svg)](https://crates.io/crates/kobito)
[![License: ISC](https://img.shields.io/badge/License-ISC-blue.svg)](LICENSE)
[![Release](https://img.shields.io/github/v/release/unhappychoice/kobito.svg)](https://github.com/unhappychoice/kobito/releases)

<p align="center">
  <img src="docs/assets/image.png" alt="kobito" style="max-width: 100%; width: 800px;" />
</p>

> Like the elves in the shoemaker's tale, it works while you sleep.

`kobito` is an autonomous coding agent orchestrator. Give it a goal, it
loops — invoking [Claude Code](https://github.com/anthropics/claude-code)
or [Codex](https://github.com/openai/codex) on your repository,
committing each iteration with an LLM-generated message that follows
the project's own conventions, until you stop it.

## Installation

### Using Install Script (Recommended)

```bash
curl -fsSL https://raw.githubusercontent.com/unhappychoice/kobito/main/install.sh | bash
```

Drops the binary in `~/.local/bin` by default. Override with
`INSTALL_DIR=/usr/local/bin` or pin a version with
`bash install.sh v0.1.0`.

### Using Homebrew

```bash
brew install unhappychoice/tap/kobito
```

### Using Cargo

```bash
cargo install kobito
```

### From Source

```bash
git clone https://github.com/unhappychoice/kobito.git
cd kobito
cargo install --path .
```

Pre-built archives for `linux x86_64 / aarch64`, `macOS x86_64 /
aarch64`, and `windows x86_64` are attached to every
[GitHub release](https://github.com/unhappychoice/kobito/releases).

### Requirements

Either the [`claude`](https://github.com/anthropics/claude-code) or
[`codex`](https://github.com/openai/codex) CLI on `PATH` (see
`--agent` below). Iteration mode additionally requires the
[`gh`](https://cli.github.com/) CLI authenticated for the project's
remote.

## Features

- `cont` mode — one branch, many commits, run until you stop it
- `iter` mode — per-task branch + PR from a `tasks.md` backlog
- Claude Code and OpenAI Codex backends behind a single `Agent` trait
- Preset system with `{{var}}` substitution (project + global)
- Per-run `notes.md` auto-maintained by the agent
- `kobito resume` with an interactive picker of recent runs
- State under `$XDG_STATE_HOME/kobito/`, event-driven streaming + status bar
- Per-iteration token usage logged from each agent's structured event stream

Project conventions — output language, code style, commit format, branch
names — are deferred to the agent's own memory files (`CLAUDE.md` /
`AGENTS.md`). kobito does not inject or pin anything itself.

## Usage

### `cont`

Pursue a single open-ended goal on one working branch:

```bash
# from inside a clean git repo
kobito cont --prompt "Increase test coverage in src/"
```

### `iter`

Consume a backlog of small tasks, one branch + PR per task:

```bash
# seed the backlog (committed to the project)
cat > .kobito/tasks.md <<'EOF'
- [ ] Add /healthz endpoint
- [ ] Wire Prometheus metrics middleware
- [ ] Document the new endpoints in README
EOF

kobito iter
```

Or point at an explicit backlog file:

```bash
kobito iter --backlog ./tasks.md
```

The first run copies `.kobito/tasks.md` (or the file passed via `--backlog`)
into the state directory. From then on the state copy is the source of truth —
edit it through:

```bash
kobito tasks edit
```

For each unchecked item, kobito branches off the starting branch as
`kobito/task-<n>-<slug>`, iterates until the agent emits `TASK_COMPLETE`,
runs `gh pr create`, and marks the line `[x]` in the state copy.

### Presets

Reuse a Markdown template across runs and projects. Place the file at:

1. `./.kobito/presets/<name>.md` — project-local override
2. `$XDG_CONFIG_HOME/kobito/presets/<name>.md` — global (defaults to `~/.config/kobito/presets/`)

Resolution checks (1) first, then (2). Missing preset → error.

`{{var}}` placeholders are substituted from `--var key=value` (repeatable). Unresolved variables abort the run before any branch is created.

```bash
# ~/.config/kobito/presets/coverage.md
# Increase test coverage for {{path}}. Aim for {{target}}% line coverage.

# preset replaces --prompt entirely:
kobito cont --preset coverage --var path=src/api --var target=80
```

In `cont` mode `--preset` is **mutually exclusive with `--prompt`** — the resolved preset body becomes the goal.

In `iter` mode each task in `tasks.md` is its own goal, so `--preset` instead acts as **framing prepended to every task prompt**:

```bash
kobito iter --preset small-feature --backlog tasks.md
```

### Resuming

```bash
# interactive picker — shows the 10 most recent runs
kobito resume

# or resume a specific run id (the timestamp dir name)
kobito resume --run 2026-05-01T12-00-00
```

`kobito resume` without `--run` opens an arrow-key picker listing the most
recent runs (id, branch, first line of the goal). When there is only one
prior run, or when stdin is not a TTY (CI, pipes), it auto-picks the
latest.

Resume re-uses the original branch and agent from the run's
`meta.json`, opens a new run directory, and copies the previous
`notes.md` as the starting memory so the loop picks up with what
the earlier iterations learned.

### Common options

| flag                | default   | meaning                                     |
| ------------------- | --------- | ------------------------------------------- |
| `--prompt`, `-p`    | required (cont) | the goal to pursue                    |
| `--backlog`         | optional (iter)  | path to a tasks.md backlog           |
| `--preset`          | optional  | preset name (resolved per the order above)  |
| `--var key=value`   | repeatable | substitute `{{key}}` in the preset         |
| `--max-iterations`  | `50` / `30` | hard cap on iterations (per task in iteration mode) |
| `--max-failures`    | `3`       | give up after N consecutive failures        |
| `--agent`           | `claude`  | `claude` (alias `claude-code`) or `codex`   |
| `--allow-dirty`     | `false`   | skip the clean-tree check                   |

## How it works

Each iteration:

1. Build a prompt: cross-iteration notes (if any) + the goal + a `NATURAL_STOP` / `TASK_COMPLETE` escape hatch.
2. Invoke the agent in non-interactive mode with its structured-output flag (`claude --output-format stream-json` / `codex exec --json`). Each line is parsed into a normalised `AgentEvent` (Message / ToolStart / ToolEnd / Usage / Stop / Other), formatted onto the terminal, and persisted both as a human-readable line in `log.ndjson` and as a raw JSON event in `events.ndjson`. The agent loads its own `CLAUDE.md` / `AGENTS.md` at this point.
3. If the agent emitted a diff, generate a Conventional Commits message via a one-shot agent call and commit.
4. If the agent failed, `git reset --hard` and retry with exponential backoff.
5. If the agent emits the literal sentinel token (`NATURAL_STOP` for `cont`, `TASK_COMPLETE` for `iter`), exit cleanly.

Single branch, single PR, many commits — by design (`cont` mode). One branch + PR per task (`iter` mode).

## State layout

```
~/.local/state/kobito/
└── projects/
    └── <repo-basename>-<sha1[:8]>/
        ├── tasks.md                # iteration backlog (state copy)
        └── runs/
            └── 2026-05-01T12-00-00/   # one directory per invocation = run id
                ├── meta.json       # branch, goal, agent — used for resume
                ├── notes.md        # cross-iteration learnings, agent-maintained
                ├── log.ndjson      # human-readable streamed lines
                ├── events.ndjson   # raw structured events from the agent
                └── prompts/
                    └── iter-0001.md
```

`notes.md` lives **per run**, not per project. Each invocation gets a
fresh memory file, and `iter` mode (which starts a new run per
task) automatically gets a separate notes.md per task — coverage
push and refactor learnings don't bleed into each other.

After every commit, the agent is asked in a one-shot call to write
1-5 short bullets distilling what is useful for the next iteration —
surprising findings, dead-ends, paths or commands worth remembering —
appended under a timestamped header. The next iteration reads the
file back into the prompt as cross-iteration memory. Empty / `NO_NOTES`
outputs are skipped.

`$XDG_STATE_HOME` is honoured if set.

## Related Projects

- [**gitlogue**](https://github.com/unhappychoice/gitlogue) — A cinematic Git commit replay tool for the terminal
- [**GitType**](https://github.com/unhappychoice/gittype) — A CLI code-typing game that turns your source code into typing challenges

## Contributing

Contributions are welcome. See [AGENTS.md](AGENTS.md) for project conventions.

## License

ISC License. See [LICENSE](LICENSE) for details.

## Author

[@unhappychoice](https://github.com/unhappychoice)
