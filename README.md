# kobito

> Like the elves in the shoemaker's tale, it works while you sleep.

`kobito` is an autonomous coding agent orchestrator. You give it a goal, and it
loops — invoking [Claude Code](https://github.com/anthropics/claude-code) on
your repository, committing each iteration with an LLM-generated
[Conventional Commits](https://www.conventionalcommits.org/) message, until you
stop it.

## Status

Early MVP. Phase 1 + 2 implemented:

All four MVP phases shipped:

- `continuous` mode end-to-end with the Claude Code agent (#2)
- `iteration` mode: per-task branch + PR from a `tasks.md` backlog (#3)
- LLM-generated commit messages (#4)
- State persisted under `~/.local/state/kobito/` (#6)
- Real-time log passthrough with a status bar (#5)
- Preset system: reusable Markdown templates with `{{var}}` substitution (#7)
- Agent abstraction with Claude Code and OpenAI Codex backends (#9)

Project conventions (style, output language) are the agent's job —
both Claude Code and Codex auto-read their respective memory files
(`CLAUDE.md` / `AGENTS.md`) at every invocation, so kobito does not
inject or pin anything itself.

## Install

```sh
cargo install --path .
```

Requires either the [`claude`](https://github.com/anthropics/claude-code)
or [`codex`](https://github.com/openai/codex) CLI on `PATH` (see
`--agent` below). Iteration mode additionally requires the
[`gh`](https://cli.github.com/) CLI authenticated for the project's
remote.

## Usage

### continuous

Pursue a single open-ended goal on one working branch:

```sh
# from inside a clean git repo
kobito continuous --prompt "Increase test coverage in src/"
```

### iteration

Consume a backlog of small tasks, one branch + PR per task:

```sh
# seed the backlog (committed to the project)
cat > .kobito/tasks.md <<'EOF'
- [ ] Add /healthz endpoint
- [ ] Wire Prometheus metrics middleware
- [ ] Document the new endpoints in README
EOF

kobito iteration
```

Or point at an explicit backlog file:

```sh
kobito iteration --backlog ./tasks.md
```

The first run copies `.kobito/tasks.md` (or the file passed via `--backlog`)
into the state directory. From then on the state copy is the source of truth —
edit it through:

```sh
kobito tasks edit
```

For each unchecked item, kobito branches off the starting branch as
`kobito/task-<n>-<slug>`, iterates until the agent emits `TASK_COMPLETE`,
runs `gh pr create`, and marks the line `[x]` in the state copy.

### presets

Reuse a Markdown template across runs and projects. Place the file at:

1. `./.kobito/presets/<name>.md` — project-local override
2. `$XDG_CONFIG_HOME/kobito/presets/<name>.md` — global (defaults to `~/.config/kobito/presets/`)

Resolution checks (1) first, then (2). Missing preset → error.

`{{var}}` placeholders are substituted from `--var key=value` (repeatable). Unresolved variables abort the run before any branch is created.

```sh
# ~/.config/kobito/presets/coverage.md
# Increase test coverage for {{path}}. Aim for {{target}}% line coverage.

# preset replaces --prompt entirely:
kobito continuous --preset coverage --var path=src/api --var target=80
```

In `continuous` mode `--preset` is **mutually exclusive with `--prompt`** — the resolved preset body becomes the goal.

In `iteration` mode each task in `tasks.md` is its own goal, so `--preset` instead acts as **framing prepended to every task prompt**:

```sh
kobito iteration --preset small-feature --backlog tasks.md
```

### Common options

| flag                | default   | meaning                                     |
| ------------------- | --------- | ------------------------------------------- |
| `--prompt`, `-p`    | required (continuous) | the goal to pursue              |
| `--backlog`         | optional (iteration)  | path to a tasks.md backlog      |
| `--preset`          | optional  | preset name (resolved per the order above)  |
| `--var key=value`   | repeatable | substitute `{{key}}` in the preset         |
| `--max-iterations`  | `50` / `30` | hard cap on iterations (per task in iteration mode) |
| `--max-failures`    | `3`       | give up after N consecutive failures        |
| `--agent`           | `claude`  | `claude` (alias `claude-code`) or `codex`   |
| `--allow-dirty`     | `false`   | skip the clean-tree check                   |

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
                ├── log.ndjson      # streamed agent output
                └── prompts/
                    └── iter-0001.md
```

`notes.md` lives **per run**, not per project. Each invocation gets a
fresh memory file, and iteration mode (which starts a new run per
task) automatically gets a separate notes.md per task — coverage
push and refactor learnings don't bleed into each other.

After every commit, the agent is asked in a one-shot call to write
1-5 short bullets distilling what is useful for the next iteration —
surprising findings, dead-ends, paths or commands worth remembering —
appended under a timestamped header. The next iteration reads the
file back into the prompt as cross-iteration memory. Empty / `NO_NOTES`
outputs are skipped.

### Resuming

```sh
# resume the most recent run on the current project
kobito resume

# or resume a specific run id (the timestamp dir name)
kobito resume --run 2026-05-01T12-00-00
```

Resume re-uses the original branch and agent from the run's
`meta.json`, opens a new run directory, and copies the previous
`notes.md` as the starting memory so the loop picks up with what
the earlier iterations learned.

`$XDG_STATE_HOME` is honoured if set.

## How it works

Each iteration:

1. Build a prompt: cross-iteration notes (if any) + the goal + a `NATURAL_STOP` / `TASK_COMPLETE` escape hatch.
2. Invoke the agent in non-interactive mode, streaming stdout/stderr to the terminal and to `log.ndjson`. The agent loads its own `CLAUDE.md` / `AGENTS.md` at this point.
3. If the agent emitted a diff, generate a Conventional Commits message via a one-shot agent call and commit.
4. If the agent failed, `git reset --hard` and retry with exponential backoff.
5. If the agent emits the literal sentinel token (`NATURAL_STOP` for continuous, `TASK_COMPLETE` for iteration), exit cleanly.

Single branch, single PR, many commits — by design (continuous mode). One branch + PR per task (iteration mode).

## License

ISC
