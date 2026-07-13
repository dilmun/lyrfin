# Contributing to lyrfin

Thanks for your interest in improving lyrfin! This guide covers the workflow, code
style, and architecture conventions the project follows.

## Getting started

You'll need **Rust 1.96+** (edition 2024). On Linux, install the ALSA dev
headers for the audio backend:

```sh
sudo apt-get install libasound2-dev   # Debian/Ubuntu
```

Then:

```sh
git clone https://github.com/dilmun/lyrfin
cd lyrfin
cargo build --profile fast   # optimized but fast to rebuild — best for dev
cargo run --profile fast -- ~/Music
```

Use the **`fast`** profile for day-to-day work: it's optimized (smooth playback
and visualizer) but skips fat LTO and stays incremental, so rebuilds relink in
seconds. Reserve `cargo build --release` for actual releases.

## Before you open a PR — keep it green

CI runs these on Linux/macOS/Windows and they must all pass:

```sh
cargo fmt --all --check                       # formatting
cargo clippy --all-targets -- -D warnings     # lint (warnings are errors)
cargo test                                    # reducer + snapshot tests
cargo run -- --snapshot --size 120x40         # headless render smoke test
```

Run `cargo fmt` (not hand-formatting) and fix every clippy finding before
committing — a clean build means **errors and warnings** clean. The only
exception is a scoped, documented `#[allow(...)]` with a one-line reason.

## Git workflow

- **Branch by intent:** `feature/…`, `fix/…`, `refactor/…`, `perf/…`, `docs/…`.
  Never commit directly to `main`.
- **Commit only when green** (tests + fmt + clippy pass).
- Keep commits focused; write clear messages (a `type(scope): summary` style is
  used across the history, e.g. `feat(ui): …`, `fix(audio): …`).
- Delete your branch after it merges.

## Architecture & code conventions

Read [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) first — lyrfin follows a strict
unidirectional flow:

```
Event  →  Action  →  AppState::update  →  render
```

- **`AppState::update` is the only mutator.** State transitions are explicit
  actions, never side effects buried in rendering.
- **Rendering only displays state** — no business logic, networking, storage, or
  Spotify logic in `ui/`.
- **Slow work goes off-thread.** Never block the UI or audio threads. Network,
  decode and scan work runs on worker threads that expose
  `(Sender<Request>, Receiver<Result>)` and are drained in the event loop.
- **Single responsibility.** Split modules by responsibility, not size. The two
  historically oversized files (`app/mod.rs`, `ui/components.rs`) are known debt —
  don't grow them; peel responsibilities out when you touch an area.
- **Error handling:** runtime paths return `Result` and propagate with `?`. No
  `unwrap`/`expect`/`panic!` in runtime logic (allowed at startup/init,
  thread-spawn that genuinely can't fail, and in tests).
- **Performance is first-class:** justify clones and allocations in hot paths;
  prefer borrowing, slices and reuse.

These conventions exist to keep the codebase understandable by a new contributor
six months later — when in doubt, match the surrounding code.

## Testing

New logic should be testable without a terminal, Spotify, database, or the real
filesystem. The reducer is a pure `(state, action) -> state` function, so most
behavior can be unit-tested directly, and rendered frames are checked with
ratatui's `TestBackend` snapshot tests (`src/snapshot/tests/`). Tests must use
temp dirs and never touch the real `~/.config/lyrfin`.

## Adding things

- **A new theme:** add a TOML file (see the format in
  [`docs/CONFIGURATION.md`](docs/CONFIGURATION.md#custom-theme-format)); no code
  needed for user themes. Built-ins live in `src/ui/theme.rs`.
- **A new source/view:** add a `Layout` variant + its state, a render fn that
  composes the shared shell (`ui/components/shell`), a `focus_order` arm, and
  `move_selection` / `activate` arms — see the "Source views" section in
  [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).
- **A dependency:** prefer fewer. Justify that it's maintained, necessary, and
  worth the compile-time/transitive cost before adding it.

## Roadmap

Planned and in-progress work is tracked in [`docs/ROADMAP.md`](docs/ROADMAP.md)
and [`docs/STATUS.md`](docs/STATUS.md). If you're picking something up, a quick
issue or note there helps avoid duplicate effort.
