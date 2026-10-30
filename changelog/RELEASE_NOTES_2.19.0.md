# Release Notes 2.19.0 (2026-10-30)

## Version 2.19.0 (2026-10-30)

- **FLARE-aware model scanning**: FLARE-engine repositories keep their
  payloads on hardware-named branches (`gb202`, `gb10`) or tags (`v1`–`v3`)
  while `main` holds metadata only — the old scan showed that empty tree.
  Selecting a FLARE model now fetches `/refs` plus each payload revision's
  file tree at its pinned commit, classifies the package (packed single
  file or `model.safetensors` + `weights/**` + `deployments/**` directory
  form) and surfaces config facts (serving context, execution ABI) and the
  publisher identity with its key fingerprint. Detection works with or
  without a `flare` tag (root `publisher.json` + config markers).
- **New `Flare` viewer mode**: revision cards (branch/tag badge, short
  commit, package file, true LFS size, parsed `tp`/`c`/`mtp` tokens, ★ on
  the revision matching the locally detected GPU), a config-facts +
  publisher panel, a `[FLARE]` badge in the results list, and mouse
  navigation — `Tab` cycles Models ↔ FLARE Revisions.
- **`download --for flare` — the `flare download` drop-in**:
  `rust-hf-downloader download JohannesBertens/Qwen3.8-Flash-Next-FLARE-1M-TP2 --for flare --gpus 0,1`
  auto-selects the branch via `nvidia-smi` PCI device IDs (flare's exact
  platform table; unknown GPUs demand `--revision`), pins every fetch, URL,
  and registry entry to the resolved commit SHA (a moved branch can never
  mix commits), downloads with the resumable engine, verifies LFS sha256s,
  and prints the serve command — `flare run '<path>' --gpus '0,1'` — so no
  flare cache install step is needed. `--include/--exclude` filter within
  the package set; the human reporter notes partial selections instead of
  printing a serve command.
- **Revision-safe destinations**: FLARE files land under
  `<output>/<author>/<model>/<revision>/` via a `remote_path`/filename
  split (unprefixed request URLs, prefixed disk paths — byte-identical
  behavior for every non-FLARE download). Pre-flight guards reject size
  mismatches, a moved branch over an existing directory (existence-based
  ownership, order-independent), and any attempt to refetch a package
  already recorded at a different destination — before a byte moves;
  output spellings through unresolvable symlinks are refused outright.
  Interrupted FLARE downloads resume at their RECORDED destination, never
  the live default directory.
- **TUI download flow parity**: `d` opens the FLARE revision picker from
  the Models list or the revisions pane; Enter opens the standard
  editable download-path popup; the chosen path flows through pre-flight,
  destinations, and the registry.
- **`hf download` interop unchanged**; `HF_ENDPOINT` mirrors work for the
  FLARE scan (unlike flare itself, which hardcodes the endpoint).
- 574 → 638 tests (offline fixtures for all three real FLARE repos, the
  URL/disk-parity interrupt-resume fixture, the six-trace destination
  ownership lifecycle table, symlink-spelling regressions).
