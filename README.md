# Pipe `|` explorer

**pipe-explorer** is an interactive terminal UI (TUI) for building and debugging shell pipelines — one stage at a time.

- 🔧 Easily construct and modify shell pipelines
- ⚡ Parse output from commands that are normally slow
- 📂 No need for using temporary files

### Example screenshot

![Video from when interactively creating a simple pipeline (`git ls-files | grep --color=always test | sort -t: | cut -d: -f1 | uniq -c`) that finds places where "test" is mentioned in a git repo and then filtering down on the number of hits in each file](./docs/demo.gif)

## Features

- **Stage-by-stage inspection** — select any stage in your pipeline and see its output immediately
- **Live editing** — add, edit, or delete pipeline stages with an inline editor
- **stdout / stderr / combined views** — switch between output streams with `1`, `2`, `3`
- **ANSI color output** — preserves terminal color sequences so colorized command output stays readable
- **Stage caching** — only change stages and their dependants are re-evaluated
- **Save to file** — write the currently viewed output to disk with `s`

## Installation

### Download a pre-built binary

Pre-built binaries for Linux, macOS, and Windows are attached to every commit in their respective build workflow. Download the archive for your platform, extract it, and place the `pipe-explorer` binary somewhere on your `PATH`.

<details><summary>Available pre-built binaries</summary>

| Platform | Asset name |
|---|---|
| Linux x86-64 (static) | `pipe-explorer-x86_64-unknown-linux-musl.tar.gz` |
| macOS Apple Silicon | `pipe-explorer-aarch64-apple-darwin.tar.gz` |
| macOS Intel | `pipe-explorer-x86_64-apple-darwin.tar.gz` |
| Windows x86-64 | `pipe-explorer-x86_64-pc-windows-msvc.zip` |

</details>

### Build from source

You need a stable [Rust toolchain](https://rustup.rs/) (Rust 1.85+).

```bash
git clone https://github.com/emanuelen5/pipe-explorer.git
cd pipe-explorer
cargo build --release
# Binary is at ./target/release/pipe-explorer
```

## Usage

Launch pipe-explorer with an optional starting pipeline (stages must be separated by a _literal_ pipe character ` \| `):

```bash
# Start new empty session
pipe-explorer

# Start with a pre-defined 2-stage pipeline
pipe-explorer find . -name '*.rs' \| wc -l

# Debug a more complex pipeline
pipe-explorer cat /var/log/syslog \| grep ERROR \| awk '{print \$5}' \| sort \| uniq -c \| sort -rn
```

If you have simpler commands, you can use the `--parse` option to automatically try to split up commands based on the location of the pipe characters:

```bash
# Same command as above, but note that we had to use " as outer quotation marks.
# Single quotes would collide with the ones in the string.
pipe-explorer --parse "find . -name '*.rs' | wc -l"
```

> [!WARNING]
> YMMV, `--parse` _only_ works for "simple" commands where you don't have
> complicated embedded strings nor ` | ` inside a string.
> You kind of have to know what you're doing.

### Keybindings

Press <kbd>?</kbd> while in the app to see all available keybindings.

<details><summary>Search</summary>

Press `/` to open the search bar at the bottom of the screen. Type a regex pattern and press `Enter` to confirm; matching text is highlighted in the output and the title bar shows the match count.

- Use `n` / `p` to jump forward / backward through all matches.
- Prefix the pattern with `\c` to search case-insensitively, or `\C` to force case-sensitive matching (default).
- Press `Esc` to clear highlights and leave search mode.

```
/ERROR\c     → case-insensitive search for "error"
/^[0-9]+     → lines that start with a number
```

</details>

## License

GPL-3.0 — see [LICENSE](LICENSE).
