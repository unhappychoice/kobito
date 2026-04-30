# kobito

> Like the elves in the shoemaker's tale, it works while you sleep.

`kobito` is an autonomous coding agent orchestrator. You give it a goal, and it
loops — invoking [Claude Code](https://github.com/anthropics/claude-code) on
your repository, committing each iteration with an LLM-generated
[Conventional Commits](https://www.conventionalcommits.org/) message, until you
stop it.

## Status

Early MVP. Phase 1 implemented:

- `continuous` mode end-to-end with the Claude Code agent (#2)
- LLM-generated commit messages (#4)
- State persisted under `~/.local/state/kobito/` (#6)
- Real-time log passthrough with a status bar (#5)
- `AGENTS.md` injection and explicit language pinning (#8)

Not yet implemented:

- `iteration` mode + `tasks.md` (#3)
- Preset system (#7)
- Codex agent backend (#9)

## Install

```sh
cargo install --path .
```

Requires the [`claude`](https://github.com/anthropics/claude-code) CLI on
`PATH`.

## Usage

```sh
# from inside a clean git repo
kobito continuous --prompt "Increase test coverage in src/"
```

Options:

| flag                | default   | meaning                                     |
| ------------------- | --------- | ------------------------------------------- |
| `--prompt`, `-p`    | required  | the goal to pursue                          |
| `--max-iterations`  | `50`      | hard cap on iterations                      |
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
