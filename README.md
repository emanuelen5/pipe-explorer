# pipe-explorer

**pipe-explorer** is an interactive terminal UI (TUI) for building and debugging shell pipelines — one stage at a time.

Instead of running a long pipe command and only seeing the final output, pipe-explorer lets you navigate between each stage of your pipeline and instantly inspect what data flows through at every step. Add, edit, or remove stages on the fly and watch the output update in real time.

```
┌─ Stage 1 ──────────────┐┌─ Stage 2 ──────────────┐┌─ Stage 3 ──────────────┐
│ cat /var/log/syslog    ││ grep "ERROR"           ││ awk '{print $5}'       │
└────────────────────────┘└────────────────────────┘└────────────────────────┘
┌─ Output (stdout) — Stage 2 ✓ ────────────────────────────────────────────────┐
│ Feb 26 10:12:05 host kernel: ERROR: unable to handle kernel NULL pointer     │
│ Feb 26 10:13:22 host app[1234]: ERROR: connection refused to 10.0.0.1:5432  │
│ Feb 26 10:14:01 host app[1234]: ERROR: retry limit exceeded                  │
└──────────────────────────────────────────────────────────────────────────────┘
 NORMAL   [q]uit  [e/Enter]edit  [n]ew  [d]el  [Tab/←/→]switch  [1]stdout …
```

## Features

- **Stage-by-stage inspection** — select any stage in your pipeline and see its output immediately
- **Live editing** — add, edit, or delete pipeline stages with an inline editor
- **stdout / stderr / combined views** — switch between output streams with `1`, `2`, `3`
- **Smart caching** — unchanged upstream stages are not re-run; only affected stages are re-executed
- **Scrollable output** — vim-style navigation (`j`/`k`, `PgDn`/`PgUp`, `g`/`G`)
- **Save to file** — write the currently viewed output to disk with `s`
- **Incremental search** — press `/` to search the output with a regex; jump between matches with `n` / `p`
- **Help overlay** — press `?` to show all keybindings at any time

## Installation

### Download a pre-built binary

Pre-built binaries for Linux, macOS, and Windows are attached to every [GitHub Release](https://github.com/emanuelen5/pipe-explorer/releases). Download the archive for your platform, extract it, and place the `pipe-explorer` binary somewhere on your `PATH`.

| Platform | Asset name |
|---|---|
| Linux x86-64 (static) | `pipe-explorer-x86_64-unknown-linux-musl.tar.gz` |
| macOS Apple Silicon | `pipe-explorer-aarch64-apple-darwin.tar.gz` |
| macOS Intel | `pipe-explorer-x86_64-apple-darwin.tar.gz` |
| Windows x86-64 | `pipe-explorer-x86_64-pc-windows-msvc.zip` |

### Build from source

You need a stable [Rust toolchain](https://rustup.rs/) (Rust 1.85+).

```bash
git clone https://github.com/emanuelen5/pipe-explorer.git
cd pipe-explorer
cargo build --release
# Binary is at ./target/release/pipe-explorer
```

## Usage

Launch pipe-explorer with an optional starting pipeline (stages separated by ` | `):

```bash
# Start empty
pipe-explorer

# Start with a pre-built pipeline
pipe-explorer "find . -name '*.rs' | wc -l"

# Debug a complex pipeline
pipe-explorer "cat /var/log/syslog | grep ERROR | awk '{print \$5}' | sort | uniq -c | sort -rn"
```

### Keybindings

| Key | Action |
|---|---|
| `Tab` / `→` / `l` | Move to the next pipeline stage |
| `Shift+Tab` / `←` / `h` | Move to the previous pipeline stage |
| `e` / `Enter` | Edit the current stage's command |
| `a` | Add a new stage after the current one |
| `n` | Add a new stage (or go to next match when search results exist) |
| `d` | Delete the current stage |
| `r` | Re-run all stages (bypass cache) |
| `s` | Save current output to a file |
| `1` / `2` / `3` | Show stdout / stderr / combined output |
| `/` | Start a regex search in the current output |
| `n` / `p` | Jump to the next / previous search match |
| `Esc` | Clear search highlights |
| `j` / `↓` | Scroll output down |
| `k` / `↑` | Scroll output up |
| `PgDn` / `Ctrl+f` | Page down |
| `PgUp` / `Ctrl+b` | Page up |
| `g` / `Home` | Jump to top of output |
| `G` / `End` | Jump to bottom of output |
| `?` | Toggle help overlay |
| `q` / `Ctrl+c` | Quit |

### Search

Press `/` to open the search bar at the bottom of the screen. Type a regex pattern and press `Enter` to confirm; matching text is highlighted in the output and the title bar shows the match count.

- Use `n` / `p` to jump forward / backward through all matches.
- Prefix the pattern with `\c` to search case-insensitively, or `\C` to force case-sensitive matching (default).
- Press `Esc` to clear highlights and leave search mode.

```
/ERROR\c     → case-insensitive search for "error"
/^[0-9]+     → lines that start with a number
```

## License

MIT — see [LICENSE](LICENSE).
