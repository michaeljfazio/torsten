//! Per-peer bandwidth limiting using a token bucket rate limiter.
//!
//! Implements a classic token bucket algorithm for controlling data transfer
//! rates on a per-peer basis. Tokens represent bytes; the bucket refills at
//! a configurable rate (bytes per second) up to a maximum burst size.

use std::time::Instant;

/// Default bandwidth limit: 50 MB/s per peer.
const DEFAULT_BYTES_PER_SEC: u64 = 50 * 1024 * 1024;

/// Token bucket rate limiter for bandwidth control.
///
/// The bucket accumulates tokens at `tokens_per_sec` rate, up to
/// `bucket_size` maximum. Each byte of data transfer consumes one token.
/// When the bucket is empty, transfers must wait for tokens to refill.
#[derive(Debug, Clone)]
pub struct TokenBucketRateLimiter {
    /// Tokens added per second
    tokens_per_sec: f64,
    /// Maximum bucket capacity (burst size)
    bucket_size: f64,
    /// Currently available tokens
    available_tokens: f64,
    /// Last time tokens were refilled
    last_refill: Instant,
}

impl Default for TokenBucketRateLimiter {
    fn default() -> Self {
        Self::new(DEFAULT_BYTES_PER_SEC)
    }
}

impl TokenBucketRateLimiter {
    /// Create a new rate limiter with the given bytes-per-second limit.
    ///
    /// The bucket size (burst capacity) is set to 2x the per-second rate,
    /// allowing short bursts while maintaining the average rate.
    pub fn new(bytes_per_sec: u64) -> Self {
        let tokens_per_sec = bytes_per_sec as f64;
        let bucket_size = tokens_per_sec * 2.0;
        TokenBucketRateLimiter {
            tokens_per_sec,
            bucket_size,
            available_tokens: bucket_size, // Start full
            last_refill: Instant::now(),
        }
    }

    /// Create a rate limiter with explicit bucket size.
    pub fn with_burst(bytes_per_sec: u64, burst_bytes: u64) -> Self {
        let tokens_per_sec = bytes_per_sec as f64;
        let bucket_size = burst_bytes as f64;
        TokenBucketRateLimiter {
            tokens_per_sec,
            bucket_size,
            available_tokens: bucket_size,
            last_refill: Instant::now(),
        }
    }

    /// Refill the bucket based on elapsed time since last refill.
    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.available_tokens =
            (self.available_tokens + elapsed * self.tokens_per_sec).min(self.bucket_size);
        self.last_refill = now;
    }

    /// Try to consume `bytes` tokens immediately.
    ///
    /// Returns `true` if there were enough tokens and they were consumed.
    /// Returns `false` if not enough tokens are available (no tokens consumed).
    pub fn try_consume(&mut self, bytes: usize) -> bool {
        self.refill();
        let needed = bytes as f64;
        if self.available_tokens >= needed {
            self.available_tokens -= needed;
            true
        } else {
            false
        }
    }

    /// Wait until enough tokens are available, then consume them.
    ///
    /// This async function sleeps for the minimum time needed for the bucket
    /// to accumulate enough tokens, then consumes them.
    pub async fn wait_for(&mut self, bytes: usize) {
        let needed = bytes as f64;
        self.refill();

        if self.available_tokens >= needed {
            self.available_tokens -= needed;
            return;
        }

        let deficit = needed - self.available_tokens;
        let wait_secs = deficit / self.tokens_per_sec;
        tokio::time::sleep(std::time::Duration::from_secs_f64(wait_secs)).await;

        self.refill();
        // After sleeping, we should have enough tokens (or close to it)
        self.available_tokens = (self.available_tokens - needed).max(0.0);
    }

    /// Current available tokens (bytes).
    pub fn available(&self) -> f64 {
        self.available_tokens
    }

    /// The configured rate in bytes per second.
    pub fn rate(&self) -> f64 {
        self.tokens_per_sec
    }

    /// The configured bucket (burst) size in bytes.
    pub fn burst_size(&self) -> f64 {
        self.bucket_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_starts_full() {
        let limiter = TokenBucketRateLimiter::new(1_000_000);
        // Bucket starts full at 2x rate
        assert!((limiter.available() - 2_000_000.0).abs() < 1.0);
    }

    #[test]
    fn test_default() {
        let limiter = TokenBucketRateLimiter::default();
        assert!((limiter.rate() - (50.0 * 1024.0 * 1024.0)).abs() < 1.0);
    }

    #[test]
    fn test_try_consume_success() {
        let mut limiter = TokenBucketRateLimiter::new(1_000_000);
        // Bucket has 2M tokens, consuming 1M should succeed
        assert!(limiter.try_consume(1_000_000));
        assert!((limiter.available() - 1_000_000.0).abs() < 1000.0);
    }

    #[test]
    fn test_try_consume_failure() {
        let mut limiter = TokenBucketRateLimiter::new(1_000);
        // Bucket has 2000 tokens, consuming 3000 should fail
        assert!(!limiter.try_consume(3_000));
        // Tokens should NOT be consumed on failure
        assert!(limiter.available() >= 1999.0);
    }

    #[test]
    fn test_try_consume_exact() {
        let mut limiter = TokenBucketRateLimiter::with_burst(1000, 1000);
        // Exact consumption
        assert!(limiter.try_consume(1000));
        // Should be at or near zero (with possible tiny refill)
        assert!(limiter.available() < 10.0);
    }

    #[test]
    fn test_with_burst() {
        let limiter = TokenBucketRateLimiter::with_burst(100, 500);
        assert!((limiter.rate() - 100.0).abs() < 0.1);
        assert!((limiter.burst_size() - 500.0).abs() < 0.1);
    }

    #[tokio::test]
    async fn test_wait_for_immediate() {
        let mut limiter = TokenBucketRateLimiter::new(10_000_000);
        // Bucket is full (20M), requesting 1M should be immediate
        let start = Instant::now();
        limiter.wait_for(1_000_000).await;
        let elapsed = start.elapsed();
        // Should complete nearly instantly (< 50ms)
        assert!(elapsed.as_millis() < 50);
    }

    #[tokio::test]
    async fn test_wait_for_delayed() {
        // 1000 bytes/sec, 1000 byte bucket
        let mut limiter = TokenBucketRateLimiter::with_burst(1000, 1000);
        // Drain the bucket
        assert!(limiter.try_consume(1000));
        // Now request 100 bytes - should wait ~100ms
        let start = Instant::now();
        limiter.wait_for(100).await;
        let elapsed = start.elapsed();
        // Should take roughly 100ms (allow 50ms-250ms for timing variance)
        assert!(elapsed.as_millis() >= 50, "elapsed: {:?}", elapsed);
        assert!(elapsed.as_millis() < 250, "elapsed: {:?}", elapsed);
    }

    #[test]
    fn test_refill_over_time() {
        let mut limiter = TokenBucketRateLimiter::with_burst(1000, 1000);
        // Drain fully
        assert!(limiter.try_consume(1000));
        // Manually set last_refill to 500ms ago to simulate time passing
        limiter.last_refill = Instant::now() - std::time::Duration::from_millis(500);
        limiter.refill();
        // Should have ~500 tokens (1000/sec * 0.5s)
        assert!(
            limiter.available() >= 400.0 && limiter.available() <= 600.0,
            "available: {}",
            limiter.available()
        );
    }

    #[test]
    fn test_refill_capped_at_bucket_size() {
        let mut limiter = TokenBucketRateLimiter::with_burst(1000, 500);
        // Even after a long time, available should not exceed bucket_size
        limiter.last_refill = Instant::now() - std::time::Duration::from_secs(100);
        limiter.refill();
        assert!(
            (limiter.available() - 500.0).abs() < 1.0,
            "available: {}",
            limiter.available()
        );
    }

    // ── Additional coverage ──────────────────────────────────────────────────

    #[test]
    fn test_try_consume_zero_bytes_always_succeeds() {
        // Consuming zero bytes should always succeed even on an empty bucket.
        let mut limiter = TokenBucketRateLimiter::with_burst(100, 100);
        // Drain the bucket
        assert!(limiter.try_consume(100));
        // Zero-byte consume must not fail
        assert!(
            limiter.try_consume(0),
            "Consuming 0 bytes must always succeed"
        );
    }

    #[test]
    fn test_burst_size_is_two_times_rate_for_default_constructor() {
        // The default constructor sets burst = 2× rate.
        let rate: u64 = 500_000;
        let limiter = TokenBucketRateLimiter::new(rate);
        assert!(
            (limiter.burst_size() - 2.0 * rate as f64).abs() < 1.0,
            "burst_size should be 2× rate; got {}",
            limiter.burst_size()
        );
    }

    #[test]
    fn test_sequential_consumes_deplete_bucket() {
        // Multiple sequential try_consume calls should reduce available tokens.
        let mut limiter = TokenBucketRateLimiter::with_burst(1000, 300);
        assert!(limiter.try_consume(100)); // 200 remaining
        assert!(limiter.try_consume(100)); // 100 remaining
        assert!(limiter.try_consume(100)); // 0 remaining (approx)
                                           // Next consume beyond ~0 should fail
        assert!(
            !limiter.try_consume(100),
            "Bucket should be exhausted after three 100-byte consumes from 300-byte bucket"
        );
    }

    #[test]
    fn test_rate_accessor_returns_configured_rate() {
        let rate: u64 = 123_456;
        let limiter = TokenBucketRateLimiter::new(rate);
        assert!(
            (limiter.rate() - rate as f64).abs() < 1.0,
            "rate() should return the configured rate"
        );
    }

    #[test]
    fn test_with_burst_custom_sizes() {
        // with_burst should use exactly the provided burst, not 2× rate.
        let limiter = TokenBucketRateLimiter::with_burst(1000, 250);
        assert!(
            (limiter.burst_size() - 250.0).abs() < 1.0,
            "burst_size should be exactly 250"
        );
        assert!(
            (limiter.available() - 250.0).abs() < 1.0,
            "Initial available tokens should equal burst_size"
        );
    }
}
