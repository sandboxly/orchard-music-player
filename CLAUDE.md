# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Orchard Music Player: a themeable local-file music player from the Orchard family. Like Orchard Commander it is a GPU-drawn desktop app (winit + wgpu) styled as a character grid, not a TUI. It reuses Commander's crates (`oc-grid` renderer, `oc-config` color schemes, `oc-rename` tag reading) as git dependencies pinned to a Commander release tag; to work on them together, add a `[patch]` section pointing at a local checkout (`../orchard-commander`).

## Commands

```sh
cargo run --release -- [FILE|FOLDER|PLAYLIST.m3u]...                      # run
cargo test                                                                # all tests
OMP_TEST_MUSIC=<folder> cargo test                                        # plus real-file decoding tests
cargo clippy --all-targets -- -D warnings                                 # lint
cargo run -- --screenshot /tmp/x.png --size 112x38 --dialog help          # look at the UI headless
```

- `ORCHARD_MUSIC_PLAYER_CONFIG_DIR=<empty dir>` runs with default settings without touching the real config and session playlist.
- Quit with ⌘Q (or F10) to save the session; a SIGTERM skips it.

## Architecture

- `app.rs` owns all state and logic (playlist, modals, filter, gapless queue, session); `ui/` draws it as a pure function into an `oc_grid::Frame`; `main.rs` owns only the window, input conversion and wake-ups.
- `audio.rs`: a decoder thread (symphonia, Opus through libopus) fills a buffer in the output's format; `audio/output.rs` owns the cpal stream on its own thread and follows the system's default device (a format change re-opens the track where it was heard).
- Blocking work (tags, covers, folder listings, M3U) goes through `tasks.rs`, never the UI thread.

## Gotchas

- **UI copy** (key bar, help, toasts, errors, empty states, prompts, hints) follows `docs/ai-microcopy-guide.md`: sentence case, front-loaded and short, errors that say what went wrong, why and how to fix it, empty states that say how to start, no periods on fragments. Read it before writing or changing user-visible text. Developer details go to the log, not into messages.
- Glyphs must exist in the grid font: emoji-presentation symbols fall back to an emoji font and break the layout (the transport uses `|◀ ▶ ❚❚ ■ ▶|`).
- Media keys need the window handle on Windows (not wired up); macOS and Linux work.
