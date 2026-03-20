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

    // ── Additional coverage ──────────────────────────────────────────────────

    #[test]
    fn test_default_constructs_same_as_new() {
        // Default impl should produce an identical result to new().
        let a = KeepAliveClient::default();
        let b = KeepAliveClient::new();
        assert_eq!(a.cookie, b.cookie);
        assert_eq!(a.state as u8, b.state as u8);
    }

    #[test]
    fn test_initial_state_is_client() {
        // A freshly created KeepAliveClient should be in StClient state.
        let client = KeepAliveClient::new();
        assert_eq!(client.state, KeepAliveState::StClient);
    }

    #[test]
    fn test_initial_cookie_is_zero() {
        // Cookie starts at 0; the first call to next_cookie returns 1.
        let client = KeepAliveClient::new();
        assert_eq!(client.cookie, 0);
    }

    #[test]
    fn test_cookie_sequential_increments() {
        // Each call to next_cookie must return the previous value + 1.
        let mut client = KeepAliveClient::new();
        for expected in 1u16..=10 {
            assert_eq!(client.next_cookie(), expected);
        }
    }

    #[test]
    fn test_cookie_wraps_around_from_zero() {
        // After wrapping at MAX, the next cookie value must be 0, then 1, ...
        let mut client = KeepAliveClient::new();
        client.cookie = u16::MAX;
        assert_eq!(client.next_cookie(), 0);
        // Continuing after wrap
        assert_eq!(client.next_cookie(), 1);
    }

    // ── KeepAlive CBOR wire format ───────────────────────────────────────────

    #[test]
    fn test_keep_alive_request_cbor_encoding() {
        // Ouroboros KeepAlive MsgKeepAlive: [0, cookie:u16]
        let cookie: u16 = 42;
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(0).unwrap(); // tag
        enc.u16(cookie).unwrap();

        let mut dec = minicbor::Decoder::new(&buf);
        let len = dec.array().unwrap().unwrap_or(0);
        assert_eq!(len, 2, "MsgKeepAlive array length must be 2");
        assert_eq!(dec.u32().unwrap(), 0, "MsgKeepAlive tag must be 0");
        assert_eq!(dec.u16().unwrap(), cookie, "Cookie value must be preserved");
    }

    #[test]
    fn test_keep_alive_response_cbor_encoding() {
        // Ouroboros MsgKeepAliveResponse: [1, cookie:u16]
        let cookie: u16 = 42;
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(1).unwrap(); // tag
        enc.u16(cookie).unwrap();

        let mut dec = minicbor::Decoder::new(&buf);
        let len = dec.array().unwrap().unwrap_or(0);
        assert_eq!(len, 2, "MsgKeepAliveResponse array length must be 2");
        assert_eq!(dec.u32().unwrap(), 1, "MsgKeepAliveResponse tag must be 1");
        assert_eq!(dec.u16().unwrap(), cookie);
    }

    #[test]
    fn test_keep_alive_done_cbor_encoding() {
        // Ouroboros MsgDone: [2]
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(1).unwrap();
        enc.u32(2).unwrap(); // tag

        let mut dec = minicbor::Decoder::new(&buf);
        let len = dec.array().unwrap().unwrap_or(0);
        assert_eq!(len, 1, "MsgDone array length must be 1");
        assert_eq!(dec.u32().unwrap(), 2, "MsgDone tag must be 2");
    }

    #[test]
    fn test_keep_alive_cookie_survives_max_value() {
        // cookie=u16::MAX should encode and decode cleanly.
        let cookie: u16 = u16::MAX;
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(0).unwrap();
        enc.u16(cookie).unwrap();

        let mut dec = minicbor::Decoder::new(&buf);
        dec.array().unwrap();
        dec.u32().unwrap();
        assert_eq!(dec.u16().unwrap(), u16::MAX);
    }
}
