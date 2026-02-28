# Pipe `|` explorer

**pipe-explorer** is an interactive terminal UI (TUI) for building and debugging shell pipelines — one stage at a time.

- 🔧 Easily construct and modify shell pipelines
- ⚡ Parse output from commands that are normally slow
- 📂 No need for using temporary files

### Example "screenshot"

```
┌ Stage 1 (1 lines) ──────────────────────────────┐┌ Stage 2 (17 lines) ────────────────────────────┐
│gh api repos/emanuelen5/pipe-explorer/commits    ││jq -r '.[] | "\(.sha): \(.commit.author.date)"' │
└────────────────────────────────────────────1/[0]┘└──────────────────────────────────────────17/[0]┘
┌ Output (stdout) — Stage 2 ────────────────────────────────────────────────────────────────────────┐
│9f60aebfb2ae2ad3a2a540f1b7c1ac2ab025955f: 2026-02-28T06:37:43Z                                     │
│c3c25ecf1823609df65310c5ed2dd28dc5ec2327: 2026-02-27T19:45:35Z                                     │
│6e89c30792950f338a0884347aa03bd30121a9f2: 2026-02-27T19:37:14Z                                     │
│e523ee6d7512b6ef940bf91ea39a392c13acddac: 2026-02-27T19:35:55Z                                     │
│2b592cb7d5eac3c23012e2f470a7cff8618f3f07: 2026-02-27T19:34:18Z                                     │
│c16f5627fcbd11f45664c7187dffae5a652e0c7f: 2026-02-27T19:24:07Z                                     │
│2aab05e1b841e60266492f9230c3c11a7faecf13: 2026-02-28T06:26:20Z                                     │
│02cbaa3dd519b4a35c0b165c32a4a6a2e1a84e53: 2026-02-27T18:27:03Z                                     │
│a9d6357277b70d1f5dc6a07eeb4117b619610a6a: 2026-02-27T17:58:27Z                                     │
│3afb93f313b5611e2848f431422481fd5d14fb5e: 2026-02-27T17:56:57Z                                     │
│b2b5bd01c7dc0900f7eea25120c19050d9073cba: 2026-02-27T11:33:30Z                                     │
│f12db0cff4bf5c48333882996bfac7b2e596f1fe: 2026-02-27T06:39:11Z                                     │
│c25f1073a1cd2872ef7007f7389277abda6c904c: 2026-02-27T06:28:37Z                                     │
│908cce749c4a8ef422403919ff86ce62804d8ffe: 2026-02-27T06:15:03Z                                     │
│fd7bd5ee4c618ee3b86dea71f38841b656b7d6e1: 2026-02-27T06:06:46Z                                     │
│f4acf759c7a7f43700fc9aba3acefb3df57d0e27: 2026-02-27T05:19:58Z                                     │
│56a4ff39621bb688b67c4be6463f8de59e86930a: 2026-02-26T21:56:49Z                                     │
│                                                                                                   │
└───────────────────────────────────────────────────────────────────────────────────────────────────┘
 NORMAL    [q]uit  [e/Enter]edit  [a]new  [d]el  [Tab/←/→]switch  [1]stdout  [2]stderr  [3]combined
```

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

Launch pipe-explorer with an optional starting pipeline (stages separated by ` | `):

```bash
# Start new empty session
pipe-explorer

# Start with a pre-defined 2-stage pipeline
pipe-explorer "find . -name '*.rs' | wc -l"

# Debug a more complex pipeline
pipe-explorer "cat /var/log/syslog | grep ERROR | awk '{print \$5}' | sort | uniq -c | sort -rn"
```

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
