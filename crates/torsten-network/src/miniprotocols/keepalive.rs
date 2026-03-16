/// KeepAlive mini-protocol
///
/// Simple ping/pong protocol to keep connections alive
/// and measure round-trip time.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum KeepAliveMessage {
    KeepAlive(u16),
    KeepAliveResponse(u16),
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names, dead_code)]
pub enum KeepAliveState {
    StClient,
    StServer,
    StDone,
}

#[allow(dead_code)]
pub struct KeepAliveClient {
    pub state: KeepAliveState,
    pub cookie: u16,
}

impl Default for KeepAliveClient {
    fn default() -> Self {
        Self::new()
    }
}

impl KeepAliveClient {
    #[allow(dead_code)]
    pub fn new() -> Self {
        KeepAliveClient {
            state: KeepAliveState::StClient,
            cookie: 0,
        }
    }

    #[allow(dead_code)]
    pub fn next_cookie(&mut self) -> u16 {
        self.cookie = self.cookie.wrapping_add(1);
        self.cookie
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cookie_increment() {
        let mut client = KeepAliveClient::new();
        assert_eq!(client.next_cookie(), 1);
        assert_eq!(client.next_cookie(), 2);
    }

    #[test]
    fn test_cookie_wraps() {
        let mut client = KeepAliveClient::new();
        client.cookie = u16::MAX;
        assert_eq!(client.next_cookie(), 0);
    }
}
