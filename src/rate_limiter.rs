use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Token bucket rate limiter for download speed control
///
/// Uses a token bucket algorithm where:
/// - Each byte downloaded requires one token
/// - Tokens refill at a configured rate (bytes/sec)
/// - Bucket has a maximum capacity (rate * burst_window)
/// - Allows short bursts above the average rate for TCP efficiency
pub struct RateLimiter {
    /// Currently available tokens
    tokens: Arc<Mutex<f64>>,

    /// Maximum tokens (bucket capacity)
    max_tokens: Arc<Mutex<f64>>,

    /// Tokens added per second (bytes/sec)
    rate: Arc<Mutex<f64>>,

    /// Last time tokens were refilled
    last_refill: Arc<Mutex<Instant>>,

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
            tokens: Arc::new(Mutex::new(max_tokens)),
            max_tokens: Arc::new(Mutex::new(max_tokens)),
            rate: Arc::new(Mutex::new(rate)),
            last_refill: Arc::new(Mutex::new(Instant::now())),
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
    pub async fn acquire(&self, bytes: usize) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Fast path: if disabled, return immediately
        if !self.enabled.load(Ordering::Relaxed) {
            return Ok(());
        }

        let requested = bytes as f64;

        loop {
            let now = Instant::now();
            self.refill(now).await;

            let mut tokens = self.tokens.lock().await;

            if *tokens >= requested {
                *tokens -= requested;
                return Ok(());
            }

            // Need to wait for tokens to refill
            let tokens_needed = requested - *tokens;
            let rate_guard = self.rate.lock().await;
            let wait_secs = tokens_needed / *rate_guard;
            drop(rate_guard);
            drop(tokens);  // Release lock before sleeping

            tokio::time::sleep(Duration::from_secs_f64(wait_secs)).await;
        }
    }

    /// Update the rate limit dynamically
    ///
    /// # Arguments
    /// * `rate_bytes_per_sec` - New rate in bytes per second
    pub async fn set_rate(&self, rate_bytes_per_sec: u64) {
        let new_rate = rate_bytes_per_sec as f64;
        let mut rate = self.rate.lock().await;
        *rate = new_rate;

        // Update max tokens based on new rate
        let new_max = new_rate * self.burst_seconds;
        drop(rate);

        let mut max_tokens = self.max_tokens.lock().await;
        *max_tokens = new_max;
        drop(max_tokens);

        // Cap current tokens to new maximum
        let mut tokens = self.tokens.lock().await;
        if *tokens > new_max {
            *tokens = new_max;
        }
    }

    /// Enable or disable rate limiting
    ///
    /// # Arguments
    /// * `enabled` - Whether to enable rate limiting
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    /// Refill tokens based on elapsed time since last refill
    async fn refill(&self, now: Instant) {
        let mut last_refill = self.last_refill.lock().await;
        let elapsed = now.duration_since(*last_refill).as_secs_f64();

        if elapsed > 0.0 {
            let rate_guard = self.rate.lock().await;
            let new_tokens = *rate_guard * elapsed;
            drop(rate_guard);

            let max_tokens_guard = self.max_tokens.lock().await;
            let max_tok = *max_tokens_guard;
            drop(max_tokens_guard);

            let mut tokens = self.tokens.lock().await;
            *tokens = (*tokens + new_tokens).min(max_tok);
            *last_refill = now;
        }
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
        assert!(elapsed >= 0.4 && elapsed <= 0.7, "Elapsed: {}", elapsed);
    }

    #[tokio::test]
    async fn test_dynamic_rate_change() {
        let limiter = RateLimiter::new(1_048_576, 2.0);  // 1 MB/s
        limiter.set_enabled(true);

        // Change rate to 2 MB/s
        limiter.set_rate(2_097_152).await;

        let start = Instant::now();

        // Use full bucket (4 MB at 2 MB/s with 2 sec burst)
        limiter.acquire(4_194_304).await.unwrap();

        // Request another 1 MB - should wait ~0.5 seconds at 2 MB/s
        limiter.acquire(1_048_576).await.unwrap();

        let elapsed = start.elapsed().as_secs_f64();

        // Should take approximately 0.5 seconds
        assert!(elapsed >= 0.4 && elapsed <= 0.7, "Elapsed: {}", elapsed);
    }

    #[tokio::test]
    async fn test_concurrent_chunks() {
        let limiter = Arc::new(RateLimiter::new(2_097_152, 2.0));  // 2 MB/s
        limiter.set_enabled(true);

        let mut handles = vec![];

        // Spawn 8 tasks each requesting 256 KB (total 2 MB)
        for _ in 0..8 {
            let lim = limiter.clone();
            handles.push(tokio::spawn(async move {
                lim.acquire(262_144).await
            }));
        }

        let start = Instant::now();
        for h in handles {
            h.await.unwrap().unwrap();
        }
        let elapsed = start.elapsed().as_secs_f64();

        // Total: 2 MB @ 2 MB/s + burst buffer = should complete in 0.5-2.0 seconds
        assert!(elapsed >= 0.3 && elapsed <= 2.5, "Elapsed: {}", elapsed);
    }

    #[tokio::test]
    async fn test_small_requests() {
        let limiter = RateLimiter::new(1_048_576, 2.0);  // 1 MB/s
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
