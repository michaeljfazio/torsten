use torsten_primitives::block::Point;

/// Block-fetch mini-protocol messages
///
/// The block-fetch protocol allows downloading ranges of blocks
/// identified by points on the chain.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum BlockFetchMessage {
    // Client messages
    RequestRange(Point, Point),
    ClientDone,

    // Server messages
    StartBatch,
    Block(Vec<u8>),
    NoBlocks,
    BatchDone,
}

/// Block-fetch state machine
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names, dead_code)]
pub enum BlockFetchState {
    /// Client has agency
    StIdle,
    /// Server has agency (streaming blocks)
    StBusy,
    /// Server is sending blocks within a batch
    StStreaming,
    /// Protocol done
    StDone,
}

#[allow(dead_code)]
pub struct BlockFetchClient {
    pub state: BlockFetchState,
}

impl Default for BlockFetchClient {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockFetchClient {
    #[allow(dead_code)]
    pub fn new() -> Self {
        BlockFetchClient {
            state: BlockFetchState::StIdle,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_client() {
        let client = BlockFetchClient::new();
        assert_eq!(client.state, BlockFetchState::StIdle);
    }
}
