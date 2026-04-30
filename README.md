# kobito

> Like the elves in the shoemaker's tale, it works while you sleep.

`kobito` is an autonomous coding agent orchestrator. You give it a goal, and it
loops — invoking [Claude Code](https://github.com/anthropics/claude-code) on
your repository, committing each iteration with an LLM-generated
[Conventional Commits](https://www.conventionalcommits.org/) message, until you
stop it.

## Status

Early MVP. Phase 1 + 2 implemented:

- `continuous` mode end-to-end with the Claude Code agent (#2)
- `iteration` mode: per-task branch + PR from a `tasks.md` backlog (#3)
- LLM-generated commit messages (#4)
- State persisted under `~/.local/state/kobito/` (#6)
- Real-time log passthrough with a status bar (#5)
- `AGENTS.md` injection and explicit language pinning (#8)

Not yet implemented:

- Preset system (#7)
- Codex agent backend (#9)

## Install

```sh
cargo install --path .
```

Requires the [`claude`](https://github.com/anthropics/claude-code) CLI on
`PATH`. Iteration mode additionally requires the [`gh`](https://cli.github.com/)
CLI authenticated for the project's remote.

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

### Common options

| flag                | default   | meaning                                     |
| ------------------- | --------- | ------------------------------------------- |
| `--prompt`, `-p`    | required (continuous) | the goal to pursue              |
| `--backlog`         | optional (iteration)  | path to a tasks.md backlog      |
| `--max-iterations`  | `50` / `30` | hard cap on iterations (per task in iteration mode) |
| `--max-failures`    | `3`       | give up after N consecutive failures        |
| `--language`        | `en`      | output language (also reads `kobito.toml`)  |
| `--agent`           | `claude`  | backend agent (only `claude` for now)       |
| `--allow-dirty`     | `false`   | skip the clean-tree check                   |

`kobito.toml` example:

```toml
language = "en"
```

## State layout

```
~/.local/state/kobito/
└── projects/
    └── <repo-basename>-<sha1[:8]>/
        ├── notes.md            # cross-iteration scratch memory (read-only to kobito today)
        ├── current-run.json    # active run metadata
        └── runs/
            └── 2026-05-01T12-00-00/
                ├── log.ndjson  # streamed agent output
                └── prompts/
                    └── iter-0001.md
```

`$XDG_STATE_HOME` is honoured if set.

## How it works

Each iteration:

1. Build a prompt: `language directive` + `AGENTS.md` (+ `CLAUDE.md` for non-Claude agents) + cross-iteration notes + the goal.
2. Invoke the agent in non-interactive mode, streaming stdout/stderr to the terminal and to `log.ndjson`.
3. If the agent emitted a diff, generate a Conventional Commits message via a one-shot agent call and commit.
4. If the agent failed, `git reset --hard` and retry with exponential backoff.
5. If the agent emits the literal token `NATURAL_STOP`, exit cleanly.

Single branch, single PR, many commits — by design.

## License

ISC
