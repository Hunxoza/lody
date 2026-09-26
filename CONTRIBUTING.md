# Contributing to Lody

Lody reads aloud what programs say, translated into your language if you like. Claude Code is
the first program it reads; the most useful thing you can add is another one. It doesn't have
to be an AI: another terminal AI (Codex CLI, Gemini CLI, Aider), a chat, a build or test log,
anything that writes text you'd rather hear.

## Add a program

A program is one file in `crates/lody-core/src/sources/` and one entry in the list at the top
of `sources/mod.rs`. The engine, the settings and the app's **Listen to** switches pick it up
from there; nothing else needs to change.

### 1. Find where the program writes what it does

Lody installs nothing into the programs it reads. Look for something the program already
writes: session logs, a history file, a local database. `claude_code.rs` follows the JSON
lines Claude Code appends to `~/.claude/projects/<project>/<session>.jsonl`.

### 2. Turn it into events

Write a `Source`: `poll` is called a few times a second and returns what happened since the
last call.

```rust
pub trait Source: Send {
    fn poll(&mut self) -> Vec<Event>;
}
```

| Event | When | What Lody does |
|---|---|---|
| `Reply { session, project, text }` | The AI finished its answer | Translates it and reads it aloud |
| `Prompt { session }` | You sent a new message | Stops reading that session |
| `Tool { session, project, tool, input }` | The AI started a tool | Says "running a command", "searching the web" (see `progress.rs`) |
| `Progress { session, project, text }` | The AI wrote something while still working | Nothing yet |

- `session` tells conversations apart when several run at once; `project` is a short name
  for where one runs (the folder name), said before the reply when more than one is active.
- Send `text` as the program wrote it, Markdown and all. Lody itself keeps code, paths, links
  and secrets out of what it translates.
- Start at the end: when Lody starts, report only what happens from then on, never the
  history. A session that begins later is read from its start.
- Skip what you don't understand rather than failing: these formats change without notice.

### 3. List it

```rust
pub const PROGRAMS: &[Program] = &[
    Program { id: "claude_code", /* ... */ },
    Program {
        id: "my_program",             // its key under [sources] in config.toml
        name: "My Program",           // shown in the app
        how: "Reads My Program's session logs (~/.my-program/logs).",
        found: || my_program::logs_dir().exists(),
        start: || Box::new(my_program::MyProgram::new(my_program::logs_dir())),
    },
];
```

### 4. Test it

Add tests next to the code with a few real lines from the program (trimmed and with nothing
private in them): one finished reply, one new prompt, one tool call, and lines that must be
skipped. `claude_code.rs` has examples, including a log being written while Lody reads it.

## Other ways to help

- **A language:** copy `crates/lody-core/locales/th.toml`, name it with the language's code
  (`ja.toml`) and translate the phrases in it.
- **A voice or a translator:** `crates/lody-core/voices.toml` and `translators.toml`.
- **Problems:** open an issue with what you did, what you expected and, from a debug build
  (`cargo run -p lody-app`), what the log says.

## Before sending a pull request

```sh
cargo fmt              # style: rustfmt.toml
cargo clippy --workspace --all-targets
cargo test
```

## How a change gets in

```
your fork: feat/codex ──pull request──▶ staging ──release──▶ main ──▶ installers + website
```

1. **Fork** Lody, and in your fork make a branch named for what it does: `feat/…` for
   something new, `fix/…`, `docs/…` or `chore/…` (`feat/codex-cli`, `fix/thai-numbers`).
2. **Open a pull request into `staging`**, not `main`; the template asks what it changes and
   how you tested it. Something big? Open an issue first, so we agree on the shape.
3. **Check** runs the commands above on Linux, Windows and macOS, and checks the branch name.
   On your first pull request it waits until the maintainer lets it run.
4. **The maintainer reviews and merges.** Anyone is welcome to review, and it helps, but only
   the maintainer can merge. It's squashed into one commit on `staging`, titled after the pull
   request: keep the title to one line that says what changes ("Read Codex CLI's session logs").
5. **Releases:** now and then the maintainer takes what has gathered on `staging` into `main`
   as one release. `main` builds the installers and updates the website; `staging` never does.

Found a security problem? Report it privately instead (see [SECURITY.md](SECURITY.md)).

Unless you say otherwise, what you contribute is licensed like Lody: MIT or Apache-2.0.

## Releasing (maintainer)

1. On `staging`, set the new version in `Cargo.toml` and `crates/lody-app/tauri.conf.json`.
2. Open a pull request from `staging` into `main` titled "Release 0.2.0", and squash-merge it.
   **Build** runs Check on all three systems, then publishes `v0.2.0` with the installers,
   and the website offers it at once. If Check fails, nothing is released: fix it on
   `staging` and merge again.
3. Put `staging` back on `main`, so the next round starts from the release:
   `git fetch origin && git push --force-with-lease origin origin/main:staging`.
