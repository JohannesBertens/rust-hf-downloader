# Release Notes - Version 2.16.0

**Release Date**: 2026-10-13

Fixes the deferred `rate-limiter-zero-rate` bug (found by a full-repo
quality audit; tracked open in `docs/DEFERRED.md` since the readability
plan). Tests 487 → 489.

## User-visible changes

- **A zero rate limit no longer breaks every download**: with rate
  limiting enabled and a rate of `0` MiB/s, every chunk task used to
  panic (`tokens_needed / 0.0` → infinite wait → `Duration` construction
  panic), so each file failed five times over with a confusing
  "chunk task failed: task panicked" error.
  - The `--rate-limit-mbps` CLI flag already rejected `0`, but the
    config file did not: `download_rate_limit_mbps = 0` (or a negative
    value, which casts to 0) with `download_rate_limit_enabled = true`
    hit the panic — including via a bare `--rate-limit` that reads the
    config value. The TUI's +/- stepper was safe (clamped to ≥ 0.1).
  - A rate of `0` now means what the documentation always said:
    **unlimited**. Checked under the limiter's state lock, so a runtime
    rate change to 0 takes effect on the very next chunk.

## Internal

- `RateLimiter::acquire` returns immediately when the state rate is
  `<= 0.0` — the limiter now honors its own documented
  `0 = unlimited` contract instead of panicking; no changes to config
  parsing, CLI validation, or the options dialog (all three paths are
  safe because the limiter itself is).
- New pins: `test_zero_rate_is_unlimited` (constructor path) and
  `test_runtime_zero_rate_is_unlimited` (the reachable `set_rate(0)`
  path, plus a positive-rate re-enable sanity check) — both panic on
  the old code.
- `docs/DEFERRED.md#rate-limiter-zero-rate` marked resolved with the
  reachability note the original entry missed.
