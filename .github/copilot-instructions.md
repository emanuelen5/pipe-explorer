# Copilot Instructions

> **Keep this file up to date.** Whenever the project structure, dependencies, or workflows change in a way that would make this file inaccurate, update it so that it remains a reliable reference.

## Project Overview

**pipe-explorer** is an interactive terminal UI (TUI) application for building and debugging shell pipelines — one stage at a time. Users can construct, modify, and inspect a multi-stage shell pipeline interactively, seeing the live output of each stage without temporary files.

Key features:
- Stage-by-stage pipeline inspection — select any stage and see its output immediately
- Live editing — add, edit, or delete pipeline stages with an inline editor
- stdout / stderr / combined views — switch output streams with `1`, `2`, `3`
- ANSI color preservation — terminal color sequences are retained in output
- Stage caching — only modified stages and their dependents are re-evaluated
- Regex search — highlight matches across output with case-sensitivity control
- Save to file — write the current output to disk with `s`

## Tech Stack

| Component | Library / Tool |
|---|---|
| Language | Rust (edition 2024, requires 1.85+) |
| Terminal UI | [ratatui](https://github.com/ratatui-org/ratatui) v0.30 |
| Terminal I/O | [crossterm](https://github.com/crossterm-rs/crossterm) v0.29 |
| Async runtime | [tokio](https://tokio.rs/) v1 (full features) |
| CLI parsing | [clap](https://docs.rs/clap) v4 (derive) |
| Regex search | [regex](https://docs.rs/regex) v1 |
| Error handling | [anyhow](https://docs.rs/anyhow) v1 |
| Caching hashes | [sha2](https://docs.rs/sha2) v0.10 |
| Byte counting | [bytecount](https://docs.rs/bytecount) v0.6.9 |

## Source Layout

```
src/
├── main.rs        # Entry point: CLI argument parsing and tokio runtime setup
├── app.rs         # Core application state, event loop, and mode management
├── editor.rs      # Inline text-editor state (EditorState, key handling, scroll)
├── executor.rs    # Shell command execution with streaming output and caching
├── ui.rs          # Terminal UI rendering (ratatui layouts, panels, dialogs)
├── pipeline.rs    # Pipeline data structures (PipeStage, Pipeline, navigation)
├── search.rs      # Regex search with history and case-sensitivity options
├── ansi.rs        # ANSI escape sequence handling and color preservation
└── tests/         # Unit tests for all major modules
```

## Common Commands

### Build

```bash
cargo build           # Debug build
cargo build --release # Release build (binary at ./target/release/pipe-explorer)
```

### Test

```bash
cargo test
```

### Format

**Always run `rustfmt` after making code changes** to ensure consistent formatting:

```bash
cargo fmt
```

Verify formatting without modifying files:

```bash
cargo fmt --check
```

### Lint

```bash
cargo clippy
```

## CI

The GitHub Actions workflow (`.github/workflows/ci.yml`) builds and tests the project on every push, and produces nightly release binaries for:

| Platform | Asset |
|---|---|
| Linux x86-64 (static musl) | `pipe-explorer-x86_64-unknown-linux-musl.tar.gz` |
| macOS Apple Silicon | `pipe-explorer-aarch64-apple-darwin.tar.gz` |
| macOS Intel | `pipe-explorer-x86_64-apple-darwin.tar.gz` |
| Windows x86-64 | `pipe-explorer-x86_64-pc-windows-msvc.zip` |
