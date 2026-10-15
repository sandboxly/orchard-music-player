# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Orchard Music Player: a themeable local-file music player from the Orchard family. Like Orchard Commander it is a GPU-drawn desktop app (winit + wgpu) styled as a character grid, not a TUI. It shares the Orchard Commons crates with Commander and the other Orchard apps (`orchard-grid` renderer, `orchard-theme` color schemes and look, `orchard-kit` tasks/input/widgets and the file picker, `orchard-metadata` tag reading) as git dependencies pinned to a Commons release tag; to work on them together, add a `[patch."https://github.com/sandboxly/orchard-commons"]` section pointing at a local checkout (`../orchard-commons`), with a path entry for every orchard-* crate, `orchard-places` included (the picker's places and bookmarks).

## Commands

```sh
cargo run --release -- [FILE|FOLDER|PLAYLIST.m3u]...                      # run
cargo test                                                                # all tests
OMP_TEST_MUSIC=<folder> cargo test                                        # plus real-file decoding tests
cargo clippy --all-targets -- -D warnings                                 # lint
cargo run -- --screenshot /tmp/x.png --size 112x38 --dialog settings      # look at the UI headless (also help, themes, scheme-editor, …)
```

- `ORCHARD_MUSIC_PLAYER_CONFIG_DIR=<empty dir>` runs with default settings without touching the real config and session playlist.
- Quit with ⌘Q (or F10) to save the session; a SIGTERM skips it.

## Architecture

- `app.rs` owns all state and logic (playlist, modals, filter, gapless queue, session); `ui/` draws it as a pure function into an `orchard_grid::Frame` with `orchard_kit::widgets`; `main.rs` plugs `App` into `orchard_shell` (the shared window, event loop, input conversion, wake-ups and command line) and adds only what the player does there: the play/pause media key as Space (which the kit leaves to the app), media keys, dropped files and the cover upload.
- `audio.rs`: a decoder thread (symphonia, Opus through libopus) fills a buffer in the output's format; `audio/output.rs` owns the cpal stream on its own thread and follows the system's default device (a format change re-opens the track where it was heard).
- Blocking work (tags, covers, M3U, expanding added folders) goes through `app.tasks` (`orchard_kit::tasks`), never the UI thread.
- Open (F2) and Load (F5) use the shared picker (`orchard_kit::picker`, `Modal::Picker`): places and Orchard Commander's bookmarks beside the listing, which it reads on its own threads; `App::process` polls it. Screenshots wait for `Picker::loading` (`--dialog browser|load|bookmarks`, `--browse DIR`).

## Gotchas

- **UI copy** (key bar, help, toasts, errors, empty states, prompts, hints) follows `docs/ai-microcopy-guide.md`: sentence case, front-loaded and short, errors that say what went wrong, why and how to fix it, empty states that say how to start, no periods on fragments. Read it before writing or changing user-visible text. Developer details go to the log, not into messages.
- Glyphs must exist in the grid font: emoji-presentation symbols fall back to an emoji font and break the layout (the transport uses `|◀ ▶ ❚❚ ■ ▶|`).
- Text fields are built with `TextField::new(..).flush().plain()`: no one-cell inset, so narrow hex fields show all of `#rrggbb`, and no paste or ⌘⌫ shortcuts (modifiers are ignored, as before the kit).
- Media keys need the window handle on Windows (not wired up); macOS and Linux work.
- **Commons lockfile**: CI builds with `--locked`, so `Cargo.lock` must record the orchard crates with their git source. A build with a local `[patch]` rewrites them as path packages: before committing the lockfile, disable the patch and run `cargo update -p orchard-grid -p orchard-theme -p orchard-kit -p orchard-metadata`. Keep the patch out of commits (the tracked `.cargo/config.toml` holds only the Windows static C runtime).
