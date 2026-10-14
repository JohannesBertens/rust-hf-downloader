//! Token-bucket rate limiter shared by the download engine: [`acquire`]
//! awaits until the bytes-per-second budget allows a transfer, and the
//! rate can be changed or disabled at runtime.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Internal state for the rate limiter (consolidated into single mutex)
struct RateLimiterState {
    tokens: f64,
    max_tokens: f64,
    rate: f64,
    last_refill: Instant,
}

impl RateLimiterState {
    fn new(tokens: f64, max_tokens: f64, rate: f64, last_refill: Instant) -> Self {
        Self {
            tokens,
            max_tokens,
            rate,
            last_refill,
        }
    }
}

/// Token bucket rate limiter for download speed control
///
/// Uses a token bucket algorithm where:
/// - Each byte downloaded requires one token
/// - Tokens refill at a configured rate (bytes/sec)
/// - Bucket has a maximum capacity (rate * burst_window)
/// - Allows short bursts above the average rate for TCP efficiency
pub struct RateLimiter {
    /// Consolidated state (tokens, max_tokens, rate, last_refill)
    state: Arc<Mutex<RateLimiterState>>,

    /// Whether rate limiting is enabled
    enabled: Arc<AtomicBool>,

    /// Burst window in seconds (fixed at 2.0)
    burst_seconds: f64,
}

impl RateLimiter {
    /// Create a new rate limiter
    ///
    /// # Arguments
    /// * `rate_bytes_per_sec` - Maximum bytes per second (0 = unlimited)
    /// * `burst_seconds` - Burst window duration (fixed at 2.0 seconds)
    pub fn new(rate_bytes_per_sec: u64, burst_seconds: f64) -> Self {
        let rate = rate_bytes_per_sec as f64;
        let max_tokens = rate * burst_seconds;

        Self {
            state: Arc::new(Mutex::new(RateLimiterState::new(
                max_tokens,
                max_tokens,
                rate,
                Instant::now(),
            ))),
            enabled: Arc::new(AtomicBool::new(false)),
            burst_seconds,
        }
    }

    /// Acquire tokens for downloading bytes
    ///
    /// Blocks until enough tokens are available. If rate limiting is disabled,
    /// returns immediately without blocking.
    ///
    /// # Arguments
    /// * `bytes` - Number of bytes to acquire tokens for
    pub async fn acquire(
        &self,
        bytes: usize,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Fast path: if disabled, return immediately
        if !self.enabled.load(Ordering::Relaxed) {
            return Ok(());
        }

        let requested = bytes as f64;

        loop {
            let mut state = self.state.lock().await;

            // A rate of 0 is the documented "unlimited" spelling
            // (docs/DEFERRED.md#rate-limiter-zero-rate, resolved): never
            // wait, never divide by zero. Checked under the lock so a
            // runtime `set_rate(0)` (a config file with
            // `download_rate_limit_mbps = 0` and limiting enabled — the
            // CLI flag validates 0 away, the config path does not) takes
            // effect on the very next acquire.
            if state.rate <= 0.0 {
                return Ok(());
            }

            // Refill tokens based on elapsed time
            let now = Instant::now();
            let elapsed = now.duration_since(state.last_refill).as_secs_f64();
            if elapsed > 0.0 {
                state.tokens = (state.tokens + state.rate * elapsed).min(state.max_tokens);
                state.last_refill = now;
            }

            // Check if we have enough tokens
            if state.tokens >= requested {
                state.tokens -= requested;
                return Ok(());
            }

            // Calculate wait time for tokens to refill
            let tokens_needed = requested - state.tokens;
            let wait_secs = tokens_needed / state.rate;
            drop(state); // Release lock before sleeping

            tokio::time::sleep(Duration::from_secs_f64(wait_secs)).await;
        }
    }

    /// Update the rate limit dynamically
    ///
    /// # Arguments
    /// * `rate_bytes_per_sec` - New rate in bytes per second
    pub async fn set_rate(&self, rate_bytes_per_sec: u64) {
        let new_rate = rate_bytes_per_sec as f64;
        let new_max = new_rate * self.burst_seconds;

        let mut state = self.state.lock().await;
        state.rate = new_rate;
        state.max_tokens = new_max;
        if state.tokens > new_max {
            state.tokens = new_max;
        }
    }

    /// Enable or disable rate limiting
    ///
    /// # Arguments
    /// * `enabled` - Whether to enable rate limiting
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_disabled_limiter() {
        let limiter = RateLimiter::new(1000, 2.0);
        limiter.set_enabled(false);

        let start = Instant::now();
        limiter.acquire(1_000_000).await.unwrap();
        let elapsed = start.elapsed().as_secs_f64();

        // Should be instant when disabled
        assert!(elapsed < 0.01);
    }

    #[tokio::test]
    async fn test_basic_rate_limiting() {
        // 1 MB/s limiter, 2 sec burst
        let limiter = RateLimiter::new(1_048_576, 2.0);
        limiter.set_enabled(true);

        let start = Instant::now();

        // Use full bucket (2 MB)
        limiter.acquire(2_097_152).await.unwrap();

        // Request another 0.5 MB - should wait ~0.5 seconds for refill
        limiter.acquire(524_288).await.unwrap();

        let elapsed = start.elapsed().as_secs_f64();

        // Should take approximately 0.5 seconds (allowing some variance)
        assert!((0.4..=0.7).contains(&elapsed), "Elapsed: {}", elapsed);
    }

    #[tokio::test]
    async fn test_dynamic_rate_change() {
        let limiter = RateLimiter::new(1_048_576, 2.0); // 1 MB/s
        limiter.set_enabled(true);

        // Change rate to 2 MB/s
        limiter.set_rate(2_097_152).await;

        // Use full bucket (4 MB at 2 MB/s with 2 sec burst)
        limiter.acquire(4_194_304).await.unwrap();

        // Request another 1 MB - should wait ~0.5 seconds at 2 MB/s
        let start = Instant::now();
        limiter.acquire(1_048_576).await.unwrap();

        let elapsed = start.elapsed().as_secs_f64();

        // Should take approximately 0.5 seconds
        assert!((0.3..=2.0).contains(&elapsed), "Elapsed: {}", elapsed);
    }

    #[tokio::test]
    async fn test_concurrent_chunks() {
        let limiter = Arc::new(RateLimiter::new(2_097_152, 2.0)); // 2 MB/s
        limiter.set_enabled(true);

        let mut handles = vec![];

        // Spawn 8 tasks each requesting 1 MB (total 8 MB)
        for _ in 0..8 {
            let lim = limiter.clone();
            handles.push(tokio::spawn(async move { lim.acquire(1_048_576).await }));
        }

        let start = Instant::now();
        for h in handles {
            h.await.unwrap().unwrap();
        }
        let elapsed = start.elapsed().as_secs_f64();

        // Total: 8 MB @ 2 MB/s with 4 MB burst should complete in ~2-4 seconds
        assert!((1.5..=4.5).contains(&elapsed), "Elapsed: {}", elapsed);
    }

    #[tokio::test]
    async fn test_zero_rate_is_unlimited() {
        // The documented contract ("0 = unlimited", see `RateLimiter::new`):
        // a constructor rate of 0 with limiting enabled must not wait —
        // and must not hit the old divide-by-zero panic in
        // `Duration::from_secs_f64(inf)` (docs/DEFERRED.md#rate-limiter-zero-rate).
        let limiter = RateLimiter::new(0, 2.0);
        limiter.set_enabled(true);

        let start = Instant::now();
        limiter.acquire(1_000_000).await.unwrap();
        let elapsed = start.elapsed().as_secs_f64();

        assert!(
            elapsed < 0.01,
            "zero rate must not wait, elapsed: {elapsed}"
        );
    }

    #[tokio::test]
    async fn test_runtime_zero_rate_is_unlimited() {
        // The reachable panic path: `download_rate_limit_mbps = 0` (or a
        // negative value, which casts to 0) in a hand-edited config file
        // with `download_rate_limit_enabled = true` reaches the limiter as
        // `set_rate(0)` + enabled — the CLI flag parser rejects 0, the
        // config path does not. Must behave as unlimited, not panic.
        let limiter = RateLimiter::new(1_048_576, 2.0);
        limiter.set_enabled(true);
        limiter.set_rate(0).await;

        let start = Instant::now();
        limiter.acquire(1_000_000).await.unwrap();
        let elapsed = start.elapsed().as_secs_f64();

        assert!(
            elapsed < 0.01,
            "runtime zero rate must not wait or panic, elapsed: {elapsed}"
        );

        // A later positive rate re-enables limiting (tokens refill from
        // the capped 0 — a small acquire must still succeed quickly).
        limiter.set_rate(1_048_576).await;
        let start = Instant::now();
        limiter.acquire(1024).await.unwrap();
        let elapsed = start.elapsed().as_secs_f64();
        assert!(
            elapsed < 1.0,
            "small acquire under a re-enabled 1 MiB/s rate must complete quickly, elapsed: {elapsed}"
        );
    }

    #[tokio::test]
    async fn test_small_requests() {
        let limiter = RateLimiter::new(1_048_576, 2.0); // 1 MB/s
        limiter.set_enabled(true);

        let start = Instant::now();

        // Make 100 small requests totaling ~100 KB (well within burst)
        for _ in 0..100 {
            limiter.acquire(1024).await.unwrap();
        }

        let elapsed = start.elapsed().as_secs_f64();

        // Should be nearly instant since it's within burst capacity
        assert!(elapsed < 0.5, "Elapsed: {}", elapsed);
    }
}
