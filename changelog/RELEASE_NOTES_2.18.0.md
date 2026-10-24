# Release Notes - Version 2.18.0

**Release Date**: 2026-10-24

Security- and correctness-hardening round from the post-2.17.0 audit:
the installer and the `update` subcommand now enforce channel trust,
manifest-supplied names are validated before anything touches disk,
oversized hub directories are no longer silently truncated, and a
corrupt registry file can no longer be wiped by the next save. Tests
490 → 532.

## User-visible changes

- **Download-channel trust rule** (audit finding 10): `RHD_DOWNLOAD_BASE`
  now accepts `https://` and `file://` only — plain `http://` is accepted
  for loopback hosts (`127.0.0.1`, `localhost`, `[::1]`) alone. A
  plain-http mirror serves both the asset and its `SHA256SUMS`, so a
  matching checksum proves nothing about the channel; the check would
  happily install the mirror's binary. Enforced in `install.sh`,
  `install.ps1` and the `update` subcommand (`update::validate_base_url`,
  a new direct `url` dependency, same pinned version). This is a behavior
  change: non-loopback http mirrors that used to work are now rejected
  with a clear error — move the mirror to https.
- **Oversized hub directories are no longer silently truncated**: the
  recursive tree walk now follows `Link: <url>; rel="next"` pagination
  (quoted, unquoted, and rel-list forms; first matching segment wins, as
  huggingface_hub does). The hub tree API serves one page per directory
  (server cap ~1000 entries); before, a larger directory yielded a file
  list that simply stopped early — missing downloads and incomplete
  `hf-cache` snapshots with no warning. Every page of every directory is
  followed, bounded by a per-directory page cap with a loud warning at
  the bound.
- **A corrupt registry file is archived, not wiped**: an unparseable
  `registry.toml` used to load as an empty registry, and the next save
  then overwrote the file — silently destroying the whole download
  history. The unreadable file is now moved to `<registry>.corrupt`
  (with a warning naming both paths) before the empty default takes
  over.
- **`update` on the same version reports "up to date" even with
  `--force`**: an identical version used to reinstall over itself (the
  phantom reinstall); `--force` now means "downgrade allowed", which is
  the only useful reading.
- **Installer robustness**: `install.ps1` restores the old binary if the
  move-in of the new one fails transiently (Defender/AV holding a handle
  used to leave neither the old nor the new exe in place) and never
  strips the shared cargo bin dir from the user `PATH` on uninstall;
  `install.sh` checks that `INSTALL_DIR` exists and is writable before
  downloading (clear error instead of a half-installed failure).

## Security

- **Manifest asset names are validated** (the single place a
  manifest-supplied name is checked): a name that is not one plain file
  component (`../`, path separators, `.`/`..`) is rejected instead of
  escaping the staging temp dir at extraction time.
- **Archive members are restricted to plain files**: a tar member that
  is a symlink or directory can no longer become the staged binary
  (`entry_type().is_file()` required; the zip extractor mirrors the
  guard with an `is_dir()` skip plus the shared 0-byte check). Every
  post-download exit path — success, extraction failure, swap failure —
  now removes the staging tree, so a failed update never leaks a
  multi-MB archive.
- **Staging happens in an exclusively-created, unpredictable temp
  dir** (a retrying `create_dir` that fails on ANY pre-existing path, so
  an attacker-planted symlink or directory at a predictable name is
  never accepted).
- **hf-cache hex-oid validation**: tree oids, commit SHAs and blob oids
  from the hub tree API must be plain hex (40/64 chars) before they name
  any path — a hostile mirror can no longer steer a snapshot path
  through `../` or `/etc`. Truncated snapshot entries (a crash between
  copy and marker) are re-copied instead of trusted.

## Internal

- `verification_buffer_size` is now capped at 16 MiB (the verification
  worker allocates the value verbatim; a hand-edited gigabyte-scale
  value used to abort the process on every verification). Zero keeps its
  documented one-byte-at-a-time read mode.
- TUI (finding 7): pressing Enter on an empty query after
  `clear_search_results()` resets the loading flag and status line
  instead of leaving a stuck spinner (char-safe truncation for blob
  short names in the CLI reporter too — byte slicing at 12 could panic
  inside a multi-byte character).
- `url` promoted to a direct dependency (same `=2.4.1` pin as the
  windows target table) for `validate_base_url`; lockfile unchanged.
- Tests: pagination parsing matrix, asset-name and hex-oid rejection
  tables, corrupt-registry archive fixture, buffer-cap matrix, force
  skip-matrix, rollback and PATH-guard scripts; one version-pinned
  snapshot regenerated for the bump.
