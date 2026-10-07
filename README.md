# Rust HF Downloader v2.14.0

A Terminal User Interface (TUI) application for searching, browsing, and downloading models from the HuggingFace model hub.

## Quick Install

**Linux / macOS** (installs or upgrades to the newest release — no admin rights needed):
```bash
curl -fsSL https://github.com/JohannesBertens/rust-hf-downloader/releases/latest/download/install.sh | sh
```

**Windows** (PowerShell 5.1 or later):
```powershell
irm https://github.com/JohannesBertens/rust-hf-downloader/releases/latest/download/install.ps1 | iex
```

Both auto-detect OS/arch, verify the SHA256 checksum, install to a user-local bin dir on your `PATH` — and **re-running either one-liner upgrades in place**, so it's always safe to re-run. Previously installed via `cargo install`? The installer detects the copy in `~/.cargo/bin` and upgrades it right there (letting `cargo uninstall` clean up its records first), so you never end up with two competing binaries. Pin a version with `sh -s -- --version vX.Y.Z` / `-Version vX.Y.Z`, or see [Installation](#installation) for all options.

## Demo

### Search & Browse
![Search & Browse Demo](docs/images/searching.gif)

### Download Flow
![Download Flow Demo](docs/images/search_download.gif)

## Features

- 🔍 **Interactive Search**: Search through thousands of HuggingFace models with popup dialog
- 🎯 **Advanced Filtering**: Sort and filter models by downloads, likes, or last modified
- ⚡ **Filter Presets**: Quick access to no-filter, popular, highly-rated, or recent models
- 💾 **Filter Persistence**: Save your preferred filter settings
- 🔐 **Gated Model Support**: Download restricted models with HuggingFace token authentication
  - Token configuration in Options screen
  - Clear error messages with helpful guidance
  - Supports Llama-3.1, Llama-2, and other gated models
- ⚙️ **Persistent Configuration**: Customize and save settings (press 'o')
  - Download directory, concurrent threads, chunk sizes
  - Retry behavior, timeout settings
  - Rate limiting with configurable speed caps
  - Verification options
  - HuggingFace authentication token
  - Settings persist across restarts
- ⌨️ **Vim-like Controls**: Efficient keyboard navigation
- 🤖 **Scriptable CLI**: One-shot `download` subcommand with human or JSON Lines output (`--json`), stable exit codes, and `HF_ENDPOINT` mirror/test override — built for scripts and AI-agent skills (see [CLI Mode](#cli-mode-one-shot-download))
- 📊 **Rich Display**: View model details including downloads, likes, and tags
- 📦 **Quantization Details**: See all available quantized versions (Q2, Q4, Q5, Q8, IQ4_XS, MXFP4, etc.) with file sizes
- 📥 **Smart Downloads**: Download models directly from the TUI with:
  - Adaptive chunk sizing for optimal performance across all file sizes
  - Configurable download speed limiting (token bucket rate limiter)
  - Real-time speed tracking with continuous updates
  - Progress tracking with per-chunk speed indicators showing actual/limit speeds
  - Remaining download size and ETA display (e.g., "Downloading (2 queued) 120GB remaining, ~45 minutes")
  - Intelligent ETA calculation based on current speed (shows minutes, rounds up conservatively)
  - Resume support for interrupted downloads
  - Multi-part GGUF file handling
  - Automatic subfolder organization by publisher/model
  - Fixed quantization folder duplication issue
  - Fixed GGUF file path duplication for subdirectory downloads
  - Download queue with status display
- ✅ **Download Tracking**: Visual indicators showing already downloaded files
- 🔒 **SHA256 Verification**: Automatic integrity checking with:
  - Post-download hash verification
  - Manual verification with 'v' key
  - Multi-part file support (all parts verified)
  - Real-time verification progress bars
  - Hash mismatch detection
- 🔄 **Resume on Startup**: Automatically detect and offer to resume incomplete downloads
- 💾 **Metadata Management**: TOML-based download registry for reliable tracking
- ⚡ **Async API**: Non-blocking UI with async API calls
- 🎨 **Colorful Interface**: Syntax-highlighted results for better readability

## Table of Contents

- [Features](#features)
- [Requirements](#requirements)
- [Installation](#installation)
- [TUI Mode (Interactive)](#tui-mode-interactive)
- [CLI Mode (One-shot Download)](#cli-mode-one-shot-download)
- [Use with vLLM / transformers (HF cache drop-in)](#use-with-vllm--transformers-hf-cache-drop-in)
- [Technical Details](#technical-details)
- [Changelog](#changelog)
- [License](#license)

## Requirements

- **Rust**: 1.75.0 or newer (compatible with Ubuntu 22.04 LTS default compiler)
- **Cargo**: Latest stable version

## Installation

### One-liner (recommended)

**Linux / macOS** (any POSIX shell — installs or upgrades to the newest
release, no admin rights needed):
```bash
curl -fsSL https://github.com/JohannesBertens/rust-hf-downloader/releases/latest/download/install.sh | sh
```

**Windows** (PowerShell 5.1 or later):
```powershell
irm https://github.com/JohannesBertens/rust-hf-downloader/releases/latest/download/install.ps1 | iex
```

Both scripts auto-detect OS and architecture, download the newest release,
verify its SHA256 checksum against the release's `SHA256SUMS`, install to a
user-local bin directory (`~/.local/bin` on Linux/macOS,
`%LOCALAPPDATA%\Programs\rust-hf-downloader` on Windows), and add it to your
`PATH` if needed. **Re-running the one-liner upgrades in place** — it is
always safe to re-run.

Useful variants:

```bash
BASE=https://github.com/JohannesBertens/rust-hf-downloader/releases/latest/download/install.sh
# pin a specific version
curl -fsSL $BASE | sh -s -- --version v2.8.0
# see what would happen without changing anything
curl -fsSL $BASE | sh -s -- --dry-run
# install somewhere else / remove it again
curl -fsSL $BASE | sh -s -- --install-dir /usr/local/bin
curl -fsSL $BASE | sh -s -- --uninstall
```

```powershell
# Windows, with options
& ([scriptblock]::Create((irm https://github.com/JohannesBertens/rust-hf-downloader/releases/latest/download/install.ps1))) -Version v2.8.0
```

### Prebuilt binaries (GitHub Releases)

Every `vX.Y.Z` tag push publishes a **GitHub Release** with prebuilt
binaries for Linux (x86_64, arm64), macOS (Apple Silicon, Intel), and
Windows (x64): grab them from the
[releases page](https://github.com/JohannesBertens/rust-hf-downloader/releases).
Assets are named by Rust target triple (e.g.
`rust-hf-downloader-x86_64-unknown-linux-gnu.tar.gz`); verify against the
release's `SHA256SUMS`, extract, and put `rust-hf-downloader` on your
`PATH`. The same files are also uploaded as workflow-run artifacts
(Actions tab), but the release assets need no GitHub login to download.

### Where your files live (per platform)

| | Linux | macOS | Windows |
|---|---|---|---|
| Config | `~/.config/jreb/config.toml` | `~/Library/Application Support/jreb/config.toml` | `%APPDATA%\jreb\config.toml` |
| Registry + default downloads | `~/models/` | `~/models/` | `%USERPROFILE%\models\` |

Two mechanisms can redirect these locations:

- **Env overrides**: `RUST_HF_DOWNLOADER_CONFIG_DIR` and
  `RUST_HF_DOWNLOADER_DATA_DIR` (config root and data root respectively;
  non-empty values only).
- **Portable mode**: place a `config.toml` next to the executable — the
  exe's folder becomes the config root and `<exe>/models/` the data root.
  Useful for running from a USB stick or an arbitrary folder.

On first run after upgrading on macOS, a config previously stored at
`~/.config/jreb/config.toml` is still read; the next save migrates it to
the new location.

### Upgrading from a `cargo install`

If you originally installed via `cargo install rust-hf-downloader`, its
binary lives in `~/.cargo/bin` (`%USERPROFILE%\.cargo\bin` on Windows) —
and because rustup *prepends* that directory to `PATH`, it would shadow a
release copy installed elsewhere. The one-liner installers handle this
automatically:

- a copy found in `~/.cargo/bin` is **upgraded in place** (same location,
  new version) after handing `cargo uninstall rust-hf-downloader` the old
  records, so `cargo install --list` stays clean
- if you have no cargo copy but `~/.cargo/bin` exists and is on `PATH`, the
  installer uses it (the cargo-binstall convention for Rust CLI tools);
  otherwise it falls back to `~/.local/bin` / `%LOCALAPPDATA%\Programs\...`
- after installing, the installer re-resolves `rust-hf-downloader` the way
  your shell would and **warns** if a different copy (Homebrew, scoop, …)
still shadows the new one, with removal instructions

### Staying up to date (`update`)

If you installed a prebuilt binary (one-liner or manual), the built-in
updater keeps it current — the one-liner installer expressed in Rust:

```bash
rust-hf-downloader update          # check, download, verify, swap in place
rust-hf-downloader update --check  # report only; exit code 70 when newer exists
rust-hf-downloader update --json   # NDJSON events for scripts
```

- Resolves the newest release from the `latest.json` manifest served at
  `releases/latest/download/latest.json` (GitHub CDN redirect — no API rate
  limits; requires ≥ v2.10.0 releases)
- Downloads the asset for your platform, **verifies the SHA256** from the
  manifest, and atomically replaces the running binary (rename on Unix;
  rename-aside on Windows)
- Mirrors: `RHD_UPDATE_BASE` overrides the release base URL entirely (same
  contract as the installers' `RHD_DOWNLOAD_BASE`)

| Exit code | Meaning |
|---|---|
| 0 | Already up to date, or update applied |
| 1 | Network/manifest/swap failure (see the message) |
| 70 | `--check` found a newer release (nothing was installed) |
| 71 | Checksum mismatch (download discarded, binary untouched) |

`cargo install` users should keep using cargo (or let the one-liner take
over, see above) — `update` replaces whatever binary it runs from.

### From source

Clone this repository:
```bash
git clone https://github.com/JohannesBertens/rust-hf-downloader.git
```

Build:
```bash
cargo build --release
```

Run the application:
```bash
cargo run --release
```

### Using Crates.io

Install:
```bash
cargo install rust-hf-downloader
```

Run:
```bash
rust-hf-downloader
```

See: [rust-hf-downloader on crates.io](https://crates.io/crates/rust-hf-downloader)

## TUI Mode (Interactive)

### Controls

#### Keyboard Controls

| Key | Action |
|-----|--------|
| `/` | Open search popup |
| `o` | Toggle options screen (configure settings) |
| `Tab` | Switch focus between Models and Quantizations lists |
| `d` | Download selected quantization (Quantizations list), whole repo (Models list, non-GGUF repos), or the selected file/folder (Repository Files tree) |
| `v` | Verify SHA256 hash of downloaded file (when Quantizations list is focused) |
| `Enter` | Execute search (in search popup) / Show details (in browse mode) / Edit directory (in options) |
| `Esc` | Close search popup / Cancel popup / Close options |
| `j` or `↓` | Move selection down in focused list / Navigate options down |
| `k` or `↑` | Move selection up in focused list / Navigate options up |
| `+` | Increment numeric option value (in options screen) / Increment focused filter |
| `-` | Decrement numeric option value (in options screen) / Decrement focused filter |
| `Space` | Toggle boolean option (in options screen) |
| `q` or `Ctrl+C` | Quit application |

#### Mouse Controls

| Action | Effect |
|--------|--------|
| **Click on panel** | Focus that panel and select first item |
| **Scroll in panel** | Navigate up/down in the focused panel |
| **Hover over panel** | Highlight panel border (cyan) |
| **Click on filter field** | Focus field and cycle to next value |
| **Scroll on filter field** | Cycle filter value up/down |

Mouse-supported panels:
- **Models list**: Click to focus, scroll to navigate models (loads details automatically)
- **Quantization Groups**: Click to focus, scroll to navigate groups
- **Quantization Files**: Click to focus, scroll to navigate files
- **File Tree**: Click to focus, scroll to navigate tree
- **Filter Toolbar**: Click/scroll on Sort, Min Downloads, or Min Likes to cycle values

#### Filter & Sort Controls
| Key | Action |
|-----|--------|
| `s` | Cycle sort field (Downloads → Likes → Modified → Name) |
| `S` (Shift+s) | Toggle sort direction (Ascending ↔ Descending) |
| `f` | Cycle focus between filter fields |
| `+` or `→` | Increment focused filter value |
| `-`, `_` or `←` | Decrement focused filter value |
| `r` | Reset all filters to defaults |
| `1` | Preset: No Filters (default) |
| `2` | Preset: Popular (10k+ downloads, 100+ likes) |
| `3` | Preset: Highly Rated (1k+ likes) |
| `4` | Preset: Recent (sorted by last modified) |
| `Ctrl+S` | Save current filter settings as defaults |

#### Resume Download Popup (on startup)
| Key | Action |
|-----|--------|
| `Y` | Resume all incomplete downloads |
| `N` | Skip incomplete downloads |
| `D` | Delete incomplete files and skip |

### How to Use

1. **Start the application**
   - App starts with empty screen - press '/' to search for models
   - If incomplete downloads exist, you'll see a resume popup first
     - Press `Y` to resume incomplete downloads
     - Press `N` to skip and continue
     - Press `D` to delete incomplete files
   
2. **Search for models** - Press '/' to search

3. **Configure settings (optional)** - Press `o` to open options screen
   - Navigate with `j`/`k`
   - Edit directory: Press Enter, type path, Enter again
   - Edit HuggingFace Token: Press Enter, paste token, Enter again (required for gated models)
   - Adjust numbers: Press `+`/`-` (including download speed limit in MB/s)
   - Toggle options: Press `+`/`-` or Space (including rate limiting enable/disable)
   - Press Esc to close and save

4. **For gated models (Llama-3.1, Llama-2, etc.)**:
   - Get a HuggingFace token from: https://huggingface.co/settings/tokens
   - Accept model terms on the model's page (e.g., https://huggingface.co/meta-llama/Llama-3.1-8B)
   - Press `o` to open options, navigate to "HuggingFace Token", press Enter, paste token, press Enter again
   - Token is saved and will be used for all future downloads

5. **Type your query** (e.g., "gpt", "llama", "mistral")

6. **Press Enter** to search

7. **Navigate model results** with `j`/`k` or arrow keys (Models list is focused by default, yellow border)

8. **View quantization details** automatically as you select different models
   - Green `[downloaded]` indicator shows files you already have

9. **Press Tab** to switch focus to the Quantizations list (yellow border moves)

10. **Navigate quantizations** with `j`/`k` or arrow keys

11. **Press `d`** to download the selected quantization:
   - A popup will appear with the default path `~/models`
   - Edit the path if needed
   - Press Enter to confirm and start download
   - Files are saved to: `{path}/{author}/{model-name}/{filename}`
   - For multi-part GGUFs, all parts are queued automatically

12. **Non-GGUF repos** (safetensors, exl2, …) show a **Repository Files** tree
    instead of quantizations: `Tab` into it, then `d` downloads the selected
    file or every file under a selected folder (`d` on the Models list grabs
    the whole repo)
   - Press Esc to cancel
   - Download progress appears in the top right corner with:
     - Progress percentage
     - Download speed (shows as "actual/limit MB/s" when rate limiting is enabled)
     - Queue count and total remaining size (e.g., "(2 queued) 120GB remaining")
     - Shows "<1GB remaining" for downloads under 1GB

12. **Press `v`** to verify a downloaded file (if SHA256 hash is available):
   - Verification runs in background with progress bar
   - Shows verification speed and percentage
   - Status shows success (✓) or hash mismatch (✗)

13. **Press Enter** to see full details of the selected item in the status bar

14. **Press Tab** again to return focus to the Models list

15. **Press `/`** to start a new search

The **Quantization Details** section shows all available GGUF quantized versions with:
- **Left**: Combined file size (formatted as GB/MB/KB) - sum of all parts for multi-part files
- **Middle**: Quantization type (Q2_K, Q4_K_M, Q5_0, Q8_0, IQ4_XS, MXFP4, etc.)
- **Right**: Filename with green `[downloaded]` indicator if already on disk

### Example Searches

- Search for GPT models: `/` → type `gpt` → `Enter`
- Search for image models: `/` → type `stable-diffusion` → `Enter`
- Search for translation models: `/` → type `translation` → `Enter`

## CLI Mode (One-shot Download)

Running the binary **without arguments launches the TUI** as before. A single
non-interactive subcommand is available for scripts, cron jobs, and AI-agent
skills:

```bash
# Search HuggingFace (query-only; table or JSON array)
rust-hf-downloader search "qwen 2.5 gguf" --sort downloads --limit 20
rust-hf-downloader search "mistral gguf" --min-downloads 1000 --json | jq '.[0].id'

# Download a specific quantization
rust-hf-downloader download bartowski/Qwen2.5-7B-GGUF --quant Q4_K_M

# Download from a specific branch or commit (repos with an empty main)
rust-hf-downloader download org/model --revision 2.0bpw --file model.gguf

# Exact file(s), whole repo, custom destination
rust-hf-downloader download org/model --file README.md --file config.json
rust-hf-downloader download org/model --all -o /data/models

# Machine-readable output for scripts and agents
rust-hf-downloader download org/model --quant Q4_K_M --json | jq -c 'select(.type=="progress")'
```

### Search

`search` is a query-only command — one bounded API call, no engine state:

```bash
rust-hf-downloader search "qwen 2.5 gguf" [--sort downloads|likes|modified|name]
                                        [--direction asc|desc]
                                        [--min-downloads N] [--min-likes N]
                                        [--limit N] [--json]
```

Unspecified flags fall back to the config defaults the TUI's filter toolbar
uses. Output rule: **queries emit one JSON document** (an array, `--json`),
**pipelines emit NDJSON events** (`download --json`) — on failure a single
`{"type":"error",…}` line is the only stdout output. A successful search
with zero results exits `0` with `[]` (scripts distinguish by array length).
The full-text `search` term itself is matched server-side by HuggingFace;
`--min-downloads`/`--min-likes` filter client-side, `--sort name` and
ascending sorts apply client-side too (the API only sorts descending).

### Selectors

| Selector | Meaning |
|---|---|
| *(none)* | Works only when the repo has exactly one downloadable file; otherwise exits 64 listing every file |
| `--quant Q4_K_M` | All files of that quantization (case-insensitive; includes every part of multi-part GGUFs) |
| `--quant mmproj` | Every multimodal-projector file (`MMPROJ`, `MMPROJ-Q8_0`, …) in one go |
| `--file PATH` | Exact repo-relative path, subdirectories included (repeatable) |
| `--all` | Everything in the repository |

Selectors are mutually exclusive. Ambiguity is never guessed: the failure
output includes the structured file list so an agent can pick a selector and
re-invoke in one round-trip.

Quantization detection walks the **whole repository tree**, so GGUFs stored
in subdirectories (e.g. a `Dynamic/` folder) are found and classified too.
Quant hints come from the filename first, then from quantization-named
directories (`Q4_K_M/…`, `MXFP4/…`); `mmproj` files always land in their own
groups, and unrecognized GGUFs appear under `OTHER` instead of vanishing.

### Other options

`-o/--output DIR` (default: config `default_directory`, usually `~/models`),
`--token TOKEN` (default `$HF_TOKEN`, then config), `--no-verify`,
`-q/--quiet`, and `HF_ENDPOINT` (base-URL override for mirrors such as
`https://hf-mirror.com` or local testing).

`--revision REV` downloads from a git branch, tag, or commit SHA instead of
`main` (issue #28) — the file listing and all resolve URLs follow the
revision, and registry entries record it so the TUI resumes from the right
source. Unknown revisions exit `64` (`not_found`).

Rate limiting can be controlled per-run without editing the config file:
`--rate-limit-mbps MBPS` enables the limiter at the given rate (e.g.
`--rate-limit-mbps 25`), `--rate-limit` enables it at the configured rate,
and `--no-rate-limit` disables it (both flags override the config file).

### Output and exit codes

Human mode prints progress to **stderr** (single-line rewrites when
interactive) and the summary to **stdout**. Single-file runs show the
file's bar, speed, and ETA; **multi-file runs** (several `--file`s,
`--all`, or `hf-cache sync`) lead with one aggregate line — files
done/total, overall %, bytes, speed, and overall ETA — with the active
file demoted to name + percent:

```text
[3/17 files 43% │ 12.63 GB/29.06 GB │ 88.0 MB/s eta 3m11s] ▸ model-00004-of-00017.safetensors 61%
```

`--progress` controls human progress output (stderr): `auto` (default)
rewrites one line on a tty and stays silent when piped; `plain` prints
one newline progress line every ~10 s regardless of tty — built for
`docker run` (no `-t`), CI logs, and `tee`, and it keeps ticking through
the verification drain (`verifying: 2 in flight, 41 verified`);
`none` disables progress entirely.

`--json` emits NDJSON events on **stdout** — `resolved`, `download_start`,
`progress` (500 ms throttle), `file_complete`, `verification_start`,
`verification_result`, `done`, plus the additive `warning` (e.g. a malformed
HF token dropped at bootstrap — never on stdout in `search --json`, which
prints only the result array) — and on failure the `error` event is
**always the last line**:

```json
{"type":"error","code":"ambiguous","message":"model has 2 downloadable file(s); …","available":[{"filename":"model-Q4_K_M.gguf","size_bytes":4947802324,"sha256":"…"}, …]}
```

| Exit code | Meaning |
|---|---|
| 0 | All files present (downloaded or already existed); verification OK or skipped |
| 1 | Download failed after retries, or a hash mismatch |
| 2 | Authentication required (`--token` / `$HF_TOKEN` / config token) |
| 64 | Usage error, unknown file, or ambiguous selection |
| 130 | Interrupted (Ctrl-C); unfinished files restart from scratch on the next run |

CLI downloads share the TUI's engine, config, and `~/models/hf-downloads.toml`
registry, so files downloaded headlessly show up in the TUI's resume/complete
views. Events and behavior are covered end-to-end by integration tests
against a mock HuggingFace server.

### Use as an agent skill

This repository ships a ready-made [Agent Skills](https://agentskills.io/specification)
skill that encodes the whole probe→select→download protocol, exit-code
decisions, and the NDJSON event schema:

```text
.agents/skills/hf-downloader/
├── SKILL.md                 # routing + core protocol + exit-code policy
├── references/
│   ├── cli-reference.md      # every flag, defaults, aliases, error codes
│   └── events.md             # full NDJSON event schema
└── scripts/
    └── hf-get.sh             # probe→select→download wrapper (needs jq)
```

**Installation** (for [pi](https://github.com/earendil-works/pi-coding-agent)
and any Agent Skills-compatible agent):

- **Inside this repo:** nothing to do — agents discover `.agents/skills/`
  from the working directory automatically.
- **Globally (this machine):** symlink or copy the directory into your
  skills location:

  ```bash
  mkdir -p ~/.agents/skills
  ln -s "$(pwd)/.agents/skills/hf-downloader" ~/.agents/skills/hf-downloader
  # or, for pi's user directory:
  ln -s "$(pwd)/.agents/skills/hf-downloader" ~/.pi/agent/skills/hf-downloader
  ```

- **From a release/crates.io unpack:** the skill is included in the package;
  copy `.agents/skills/hf-downloader` out of the extracted archive the same
  way.

Verify with `pi` startup diagnostics or `/skill:hf-downloader`, then try:

```bash
.agents/skills/hf-downloader/scripts/hf-get.sh bartowski/Qwen2.5-7B-GGUF
```

The skill versions with the binary because the exit codes and event schema
are contracts — keep the installed skill in sync with the CLI version
(>= 2.3.0).

## Use with vLLM / transformers (HF cache drop-in)

`hf-cache sync` populates the **real** HuggingFace hub cache — the exact
`~/.cache/huggingface/hub` layout `huggingface_hub`, transformers, and vLLM
already read — using this tool's chunked parallel downloads, SHA256
verification, and rate limiting. After a sync, serving stacks resolve a
revision-pinned snapshot from local disk and make **zero network calls**:

```bash
# Fetch only what vLLM reads (configs + tokenizer + safetensors), pinned to a commit
rust-hf-downloader hf-cache sync Qwen/Qwen2.5-7B-Instruct --for vllm --revision <commit-sha>

# Serve fully offline — vLLM finds the model in the hub cache
HF_HUB_OFFLINE=1 vllm serve Qwen/Qwen2.5-7B-Instruct
```

The same cache works for transformers:
`HF_HUB_OFFLINE=1 AutoModel.from_pretrained("Qwen/Qwen2.5-7B-Instruct")`,
and for `hf download` / `hf cache` tooling, which resume from or scan the
blobs we wrote. `rust-hf-downloader hf-cache path <MODEL_ID> [--revision
<REV>]` prints the snapshot directory for a model (pure path math — no
network), handy for pointing other tools at the right place.

### What gets written

The standard hub layout, byte-compatible with what `hf download` would
produce (LFS blobs named by sha256, non-LFS by git blob sha1, snapshot
entries as relative symlinks so the cache survives any mount point — on
Windows, where relative symlink targets need developer mode and backslash
separators, the tool automatically uses hub's no-symlink cache mode
instead: snapshot entries are real files, byte-identical to read):

```text
~/.cache/huggingface/hub/                          # default cache root
├── CACHEDIR.TAG                                   # backup tools skip the cache
└── models--Qwen--Qwen2.5-7B-Instruct/
    ├── refs/main                    # 40-hex commit SHA of the synced revision
    ├── blobs/<oid>                  # content-addressed: sha256 (LFS) | git sha1
    └── snapshots/<commit-sha>/      # what vLLM/transformers resolve
        ├── config.json                       -> ../../blobs/<oid>
        ├── tokenizer.json                    -> ../../blobs/<oid>
        └── model-00001-of-00004.safetensors  -> ../../blobs/<oid>
```

Files are downloaded into a staging area inside the repo folder and
**published only after bytes are complete and digests check out**:
LFS files must pass the SHA256 gate, non-LFS files are hashed and checked
against the tree's git oid. Publishing is `rename(2)` into `blobs/<oid>`,
then the snapshot symlink. A concurrently running offline reader never
observes partial files, and a re-run of the same sync is a network-free
no-op (blob present + size match ⇒ skip; missing symlink ⇒ relink; branch
moved ⇒ new snapshot fetched, `refs/` updated, old snapshot kept).
Failed files are simply re-fetched from scratch on the next run — keep
`--force` for cases where you suspect corrupt local state.

### Command surface

```text
rust-hf-downloader hf-cache sync <MODEL_ID> [FILE…]      # FILE = repo-relative paths
    --revision <REV>        # branch | tag | 40-hex SHA   (default: main)
    --for <PRESET>          # "vllm" (see below)
    --include <GLOB>        # repeatable, hf-download semantics (fnmatch)
    --exclude <GLOB>        # repeatable
    --cache-dir <DIR>       # default: $HF_HUB_CACHE > $HF_HOME/hub > ~/.cache/huggingface/hub
    --no-symlinks           # copy files into snapshots/ (default on Windows)
    --force                 # re-download even if the blob already exists
    --dry-run               # list what would be fetched/skipped; no writes
    --token / --json / --quiet / --no-verify /
    --rate-limit[-mbps] / --no-rate-limit                 # existing shared flags

rust-hf-downloader hf-cache path <MODEL_ID> [--revision <REV>] [--cache-dir <DIR>]
```

Selection precedence (deterministic): positional `FILE…` → `--include` /
`--exclude` (Python-`fnmatch` semantics, `*` crosses `/`) → `--for vllm`
preset → whole repository (with a hint that `--for vllm` exists).
`--exclude` additionally filters the result of every pattern-based mode
(include, preset, whole-repo); explicitly listed `FILE…` arguments are
taken literally and never filtered.
`--exclude` applies on top of every mode. The `--for vllm` preset fetches
`*.safetensors *.json *.txt *.model *.jinja` (sharded safetensors in
subfolders included) and skips everything vLLM never reads (`original/**`,
`*.bin`, `*.pt`, `*.gguf`, `*.onnx`, …).

On success the human-mode summary's **last line is the snapshot path**
(hf-CLI parity), ready for `$(…)`. `--json` streams the usual NDJSON events
plus `SyncPlanned { files, skipped, total_bytes }`, `FilePublished { path,
blob }`, and `SyncComplete { snapshot_path, revision, sha }`. Exit codes:
`0` success including a
fully-cached no-op, `64` bad model id / empty selection / unknown revision,
`1` download or verification failure — a hash mismatch never publishes into
the cache.

### Container patterns

**A — shared-volume prefetch (lab default).** An init container (or a
docker pre-step) runs `hf-cache sync <MODEL> --for vllm --revision <sha>
--cache-dir /hf` into a volume; the server mounts that volume read-only
with `HF_HUB_OFFLINE=1`. Only the prefetcher ever writes — readers are
read-only, so there is no lock contention even on NFS. See the commented
example: [examples/k8s/vllm-prefetch.yaml](examples/k8s/vllm-prefetch.yaml)
(includes the PVC variant for multi-node labs).

**B — baked image (immutable deployments).** A multi-stage Dockerfile
`COPY`s a locally built release binary, syncs the model at **build** time
(gated-repo tokens via BuildKit `--mount=type=secret`), and the final
`vllm/vllm-openai` stage ships the cache with `HF_HUB_OFFLINE=1` baked in —
zero pulls at deploy time, exact reproducible weights per image digest. See
[examples/docker/Dockerfile.baked](examples/docker/Dockerfile.baked).

### Gotchas

> - **When overriding the cache location, set *both* `HF_HOME` and
>   `HF_HUB_CACHE`.** Different libraries consult different variables
>   (vLLM, transformers, and `huggingface_hub` versions disagree); setting
>   only one leaves the other pointing at an empty default and the server
>   silently re-downloads or fails offline. The example files set both.
> - **Single writer per cache.** Only one process should ever write a shared
>   cache volume — the prefetcher/init container. Readers must mount
>   read-only. Multiple concurrent writers (especially on NFS) are
>   unsupported.
> - **Readers: read-only mount + `HF_HUB_OFFLINE=1`.** Offline mode is what
>   turns the cache into a guarantee (no network fallback, no partial
>   re-fetches). Without it, a missing file silently goes to the network —
>   exactly what air-gapped deployments must avoid.

## Technical Details

### Architecture

- **Rust Edition**: 2021
- **Minimum Rust Version**: 1.75.0+ (Ubuntu 22.04 compatible)
- **TUI Framework**: [ratatui](https://github.com/ratatui/ratatui)
- **HTTP Client**: reqwest with async support and streaming downloads
- **TLS Backend**: rustls (pure Rust TLS implementation)
- **API**: HuggingFace REST API (`https://huggingface.co/api/models`); override the base URL with `HF_ENDPOINT` (mirrors, testing)
- **Text Input**: tui-input for search box handling
- **Download Management**:
  - Adaptive chunk sizing (targets ~20 chunks per file, 5MB-100MB range)
  - Parallel downloads with up to 8 concurrent chunks
  - Token bucket rate limiting with 2-second burst window
  - Real-time speed tracking (updated every 200ms during streaming)
  - TOML-based metadata registry (`~/models/hf-downloads.toml`)
  - Automatic resume from byte position
  - Retry logic with exponential backoff
  - Multi-part file detection and grouping
  - In-memory tracking of completed downloads

### API Integration

The application queries the HuggingFace API with the following parameters:
- Search query from user input
- Results limited to 50 models
- Sorted by downloads in descending order

### Project Structure

```
rust-hf-downloader/
├── Cargo.toml              # Dependencies and project metadata
├── README.md               # This file
├── changelog/              # Release notes for all versions
└── src/
    ├── main.rs             # Entry point (TUI by default; `download` subcommand dispatch)
    ├── cli/                # One-shot CLI subcommands (download/search/update/hf-cache), split by section:
    │   ├── mod.rs          # Cli/Command clap roots, run() dispatcher
    │   ├── args.rs         # Argument structs + parse/merge helpers
    │   ├── resolve.rs      # File selection/resolution (pure)
    │   ├── events.rs       # Stable additive-only NDJSON event schema
    │   ├── report.rs       # Human/JSON reporters, --progress modes
    │   ├── download_cmd.rs # download orchestration (run_download)
    │   ├── run.rs          # cross-command runner: RunTally/monitor/poll_once drain, shared bootstrap helpers
    │   ├── search_cmd.rs   # search subcommand
    │   ├── hf_cache/       # hf-cache group: selection (pure) / sync (pipeline) / path
    │   ├── update_cmd.rs   # self-update subcommand
    │   └── tests.rs        # cli::tests — insta snapshots in src/cli/snapshots/
    ├── engine/            # Shared download engine behind a facade: mod.rs (EngineState), enqueue.rs (sealed EnqueuePolicy), workers.rs (manager+verification spawns), bootstrap.rs
    ├── models/            # Shared data types behind a facade: api/ui/engine/options/cache
    ├── paths.rs           # Cross-platform config/registry/download path resolution
    ├── fmt.rs             # Human formatting (file sizes, durations, speeds; per-surface wrappers — see fmt.rs contract)
    ├── cache_layout.rs    # HuggingFace hub cache layout writer (cache_layout: the on-disk shape; cli/hf_cache/ is the command group)
    ├── patterns.rs        # Python-fnmatch parity glob matcher (--include/--exclude)
    ├── update.rs          # Self-update backend (latest.json manifest + verified swap)
    ├── config.rs          # Configuration persistence
    ├── utils.rs           # Streaming-digest + atomic-rename primitives
    ├── api/               # HuggingFace API client behind a facade: client/quant/tree
    ├── http_client.rs     # Authenticated HTTP requests (shared per-run client)
    ├── registry.rs        # Download registry persistence (typed mutation ops)
    ├── download/          # Download transport: mod.rs (phase decomposition) + chunked.rs (chunk workers, JoinSet abort)
    ├── rate_limiter.rs    # Token bucket rate limiter
    ├── verification.rs    # SHA256 verification worker
    └── ui/
        ├── mod.rs         # UI module declaration + re-exports
        ├── tree.rs        # File-tree navigation model (flatten/toggle/count, zero-clone traversal)
        ├── render/        # TUI rendering (facade + one file per panel; pure pass returning hit-rects)
        └── app/           # App submodule: run loop + draw + mouse + events/
            ├── mod.rs         # App + run loop + mouse handlers
            ├── state.rs       # App state container (render cache, mouse, options dialog state)
            ├── filters.rs     # FilterState: filter/sort values + mutation rules
            ├── events/        # events/mod.rs (dispatch router) + keys.rs (key handlers)
            ├── search.rs      # Model browsing logic (search, details, quantizations)
            ├── downloads.rs   # Download management
            └── verification.rs # Verification UI
```

## Dependencies

- `ratatui`: TUI framework
- `crossterm`: Terminal manipulation
- `tokio`: Async runtime
- `reqwest`: HTTP client with streaming support
- `serde`: JSON serialization
- `tui-input`: Text input widget
- `color-eyre`: Error handling
- `toml`: TOML serialization for download metadata
- `regex`: Multi-part filename pattern matching
- `urlencoding`: URL-safe query encoding
- `futures`: Async stream utilities
- `sha2`: SHA256 hash calculation
- `hex`: Hex encoding for hash display
- `once_cell`: Lazy static initialization for rate limiter

## Security

Key security features in v0.6.0:
- ✅ Path traversal protection with comprehensive validation
- ✅ Sanitization of all user inputs and API responses
- ✅ Canonicalization checks for download paths

## Changelog

| Version | Date | Summary |
|---------|------|---------|
| [2.10.0] | 2026-09-28 | `update` subcommand: self-update with SHA256 verification and atomic swap (`latest.json` manifest, `RHD_UPDATE_BASE` mirrors); TUI version badge; docs audit |
| [2.9.0] | 2026-09-28 | Installers take over `cargo install` copies in `~/.cargo/bin` (in-place upgrade + `cargo uninstall` handoff); cargo-binstall bin convention; shadow detection |
| [2.8.0] | 2026-09-28 | GitHub Releases with prebuilt binaries for 5 target triples + `SHA256SUMS`; one-liner installers `install.sh` / `install.ps1` (upgrades in place) |
| [2.7.0] | 2026-09-26 | `--revision` branch/tag/SHA downloads (#28); `--rate-limit`/`--rate-limit-mbps` CLI flags (#26); 404s map to `not_found` |
| [2.6.1] | 2026-09-26 | Fix flaky e2e temp-home collision; rename retry for transient FS locks (#37) |
| [2.6.0] | 2026-09-26 | Cross-platform paths: dirs-based config/registry/download roots, env overrides, portable mode, Windows-safe path sanitization |
| [2.5.0] | 2026-09-25 | CI: tag-triggered release builds on Linux/macOS/Windows runners |
| [2.4.0] | 2026-09-25 | Quant detection from full recursive tree: subdirectory GGUFs, `MMPROJ` groups, `mxfp4_moe`, `OTHER` fallback; per-file/folder tree downloads (#25) |
| [2.3.0] | 2026-09-24 | CLI: one-shot `download` + query-only `search` subcommands (JSON output, exit codes, HF_ENDPOINT); shared download engine extracted |
| [2.2.0] | 2026-09-24 | Activity HUD: compact matrix queue view (chunks, parallel verification, queue) |
| [2.1.0] | 2026-09-24 | Verification: truthful progress/ETA, blocking-thread hashing, 4-way concurrency |
| [2.0.0] | 2026-09-23 | Breaking: removed headless/CLI mode; TUI-only binary; snapshot test suite |
| [1.4.0] | 2026-02-13 | Optimized verification progress with AtomicU64 and cache Entry API |
| [1.3.2] | 2026-01-21 | Exact model match for repository ID searches |
| [1.3.1] | 2026-01-21 | Added F16 and TQ quantization support |
| [1.2.2] | 2026-01-08 | Fixed color contrast on light terminals |
| [1.2.1] | 2026-01-07 | Download progress shows total remaining size |
| [1.2.0] | 2026-01-07 | Download speed rate limiting (token bucket) |
| [1.1.1] | 2025-12-16 | Fixed GGUF path duplication for subdirectories |
| [1.0.0] | 2025-11-27 | Removed trending models, search-only startup |
| [0.9.7] | 2025-11-25 | Fixed file path handling bugs |
| [0.9.5] | 2025-11-25 | HuggingFace token authentication for gated models |
| [0.9.0] | 2025-11-25 | Persistent configuration system with options screen |
| [0.8.0] | 2025-11-23 | SHA256 hash verification system |
| [0.7.5] | 2025-11-23 | Adaptive chunk sizing for downloads |
| [0.7.0] | 2025-11-21 | Complete modular architecture overhaul |
| [0.6.2] | 2025-11-21 | Switched to rustls for TLS |
| [0.6.0] | 2024-11-21 | Fixed path traversal vulnerability |

See [changelog/README.md](changelog/README.md) for detailed release notes.

## License

Copyright (c) Johannes Bertens

This project is licensed under the MIT license ([LICENSE] or <http://opensource.org/licenses/MIT>)

[LICENSE]: ./LICENSE
