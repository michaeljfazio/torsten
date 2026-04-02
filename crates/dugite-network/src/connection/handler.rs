//! Connection handler — manages protocol tasks for a single connection.
//!
//! Orchestrates which protocol tasks run on a connection based on its
//! temperature (warm = keepalive only, hot = all protocols).

use tokio_util::sync::CancellationToken;

/// Per-connection protocol orchestrator.
///
/// Manages the lifecycle of protocol tasks (ChainSync, BlockFetch, etc.)
/// running on a single peer connection. Uses CancellationTokens for
/// graceful shutdown with a 5-second timeout.
pub struct ConnectionHandler {
    /// Cancellation token for all warm-tier protocols (KeepAlive).
    warm_cancel: CancellationToken,
    /// Cancellation token for all hot-tier protocols (ChainSync, BlockFetch, etc.).
    hot_cancel: CancellationToken,
    /// Whether hot protocols are currently running.
    hot_running: bool,
}

/// Timeout for graceful protocol shutdown.
const SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

impl ConnectionHandler {
    /// Create a new connection handler.
    pub fn new() -> Self {
        Self {
            warm_cancel: CancellationToken::new(),
            hot_cancel: CancellationToken::new(),
            hot_running: false,
        }
    }

    /// Get the cancellation token for warm protocols (KeepAlive).
    pub fn warm_cancel_token(&self) -> CancellationToken {
        self.warm_cancel.clone()
    }

    /// Get the cancellation token for hot protocols.
    pub fn hot_cancel_token(&self) -> CancellationToken {
        self.hot_cancel.clone()
    }

    /// Signal hot protocols to stop (demote from hot → warm).
    /// Sends cancellation and waits up to 5 seconds for graceful shutdown.
    pub async fn stop_hot_protocols(&mut self) {
        if self.hot_running {
            self.hot_cancel.cancel();
            tokio::time::sleep(SHUTDOWN_TIMEOUT).await;
            // Reset the token for potential re-promotion
            self.hot_cancel = CancellationToken::new();
            self.hot_running = false;
        }
    }

    /// Mark hot protocols as running (after promotion from warm → hot).
    pub fn mark_hot_running(&mut self) {
        self.hot_running = true;
    }

    /// Signal all protocols to stop (disconnect).
    pub async fn stop_all(&mut self) {
        self.hot_cancel.cancel();
        self.warm_cancel.cancel();
        tokio::time::sleep(SHUTDOWN_TIMEOUT).await;
    }

    /// Whether hot protocols are currently running.
    pub fn is_hot(&self) -> bool {
        self.hot_running
    }

    /// The shutdown timeout duration.
    pub fn shutdown_timeout() -> std::time::Duration {
        SHUTDOWN_TIMEOUT
    }
}

impl Default for ConnectionHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lifecycle() {
        let mut handler = ConnectionHandler::new();
        assert!(!handler.is_hot());

        handler.mark_hot_running();
        assert!(handler.is_hot());

        // Tokens should be distinct
        let warm = handler.warm_cancel_token();
        let hot = handler.hot_cancel_token();
        assert!(!warm.is_cancelled());
        assert!(!hot.is_cancelled());
    }
}
