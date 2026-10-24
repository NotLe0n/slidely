# AGENTS.md

slidely is a GPUI desktop app (through `gpui-kit`) for studying recorded
lectures. A project holds the slides (PDFs) and recordings (videos) of a
lecture. The videos are transcribed with whisper.cpp (`whisper-rs`, Vulkan),
and the transcript is matched to the slide text, so each slide can be found in
the videos. See `README.md` for what users see and how to install it.

## Building and testing

```sh
cargo build
cargo clippy
cargo test
cargo fmt
```

- Dependencies are built with `opt-level = 3` even in dev builds
  (`Cargo.toml`), so the first build is slow, but debug runs are fast enough to
  use.
- **Windows:** the target is `x86_64-pc-windows-gnu` with MSYS2 at
  `C:\msys64`. `C:\msys64\mingw64\bin` must come first on `PATH`, both for
  building (pkg-config, GStreamer, Vulkan) and for running (DLLs). In Git Bash,
  prefix commands with `export PATH="/c/msys64/mingw64/bin:$PATH";`. If
  whisper.cpp is compiled by a different MinGW gcc than the one that ships the
  runtime DLLs, the app crashes with `STATUS_ENTRYPOINT_NOT_FOUND`
  (`0xc0000139`).
- `.cargo/config.toml` and `cmake/windows-gnu.cmake` work around quirks in how
  whisper-rs-sys builds on windows-gnu. The comments there say why. Don't
  remove them without building on Windows.
- Models live in `models/` (gitignored), relative to the working directory.
  `SLIDELY_WHISPER_MODEL` and `SLIDELY_VAD_MODEL` override the paths. See
  `src/video/transcribe.rs`.
- To check the slide matching on a real lecture, run the ignored test on a
  saved project:
  `SLIDELY_ALIGN_PROJECT=path/to.slidely cargo test print_alignment -- --ignored --nocapture`

## Layout

| Path | What's in it |
| --- | --- |
| `src/main.rs` | App setup, the global `Settings` (view visibility, follow modes) |
| `src/menus.rs` | Actions and the menu bar |
| `src/pdf/` | Loading, rasterizing (`hayro`) and extracting text from PDFs |
| `src/video/` | Loading videos (GStreamer), transcription (`transcribe.rs`) |
| `src/align/` | Matching transcripts to slides: windowed word similarity (`index.rs`) and a Viterbi walk (`viterbi.rs`) |
| `src/project/` | The project model, its JSON `.slidely` file (`file.rs`), keeping the alignment up to date (`alignment.rs`) |
| `src/ui/` | All views and windows |
| `src/observe.rs`, `src/interval.rs` | Small helpers: observing each entity in a list, running a callback on an interval |

`pdf`, `video`, `align` and `project` don't render UI, and their module docs
say so. Keep it that way: rendering belongs in `src/ui/`. `align` is pure
computation over `PageText` and `Segment` and doesn't depend on GPUI.

Slow work (rasterizing, text extraction, transcription, alignment) runs off
the main thread: on `cx.background_executor()`, with `rayon` for PDF pages, and
on plain `std::thread`s for loading the Whisper models. What it produces is
saved in the project file, so it isn't done again.

## Code style

- Format with `rustfmt` defaults (4 spaces in Rust). Other files, such as
  `Cargo.toml`, use tabs (`.zed/settings.json`).
- Every module starts with a `//!` doc saying what it's for. Items get `///`
  docs that say what they do and why, in plain sentences
  (e.g. "Calls `f` every `interval` until the entity is dropped, or the task
  is."). Tunable constants get a doc explaining the value. Match that density
  and tone: plain words, explain the why, don't restate the code.
- Errors use `anyhow` with `.context(...)`. Errors in background work that
  don't stop the app are printed with `eprintln!("...: {err:#}")`.
- Use the existing helpers before writing new ones: `ObservedDocuments`
  (`src/observe.rs`), `spawn_interval` (`src/interval.rs`), the shared controls
  in `src/ui/controls.rs`, and the placeholders in `src/ui/placeholders.rs`.
- When you add a menu action, define it in `src/menus.rs` with the others, and
  disable it through `MenuState` when there's nothing for it to act on.

## Commits

Short, imperative subject lines (e.g. "Add slide-video matching"). Commit only
when asked.
