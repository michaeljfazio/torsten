use std::net::SocketAddr;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ServerError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Server already running")]
    AlreadyRunning,
    #[error("Bind failed: {0}")]
    BindFailed(String),
}

/// Node network server configuration
#[derive(Debug, Clone)]
pub struct NodeServerConfig {
    /// TCP listen address for node-to-node connections
    pub listen_addr: SocketAddr,
    /// Unix domain socket path for node-to-client connections
    pub socket_path: PathBuf,
    /// Maximum number of concurrent connections
    pub max_connections: usize,
}

impl Default for NodeServerConfig {
    fn default() -> Self {
        NodeServerConfig {
            listen_addr: std::net::SocketAddr::from(([0, 0, 0, 0], 3001)),
            socket_path: PathBuf::from("node.sock"),
            max_connections: 200,
        }
    }
}

/// The node's network server
pub struct NodeServer {
    config: NodeServerConfig,
    is_running: bool,
}

impl NodeServer {
    pub fn new(config: NodeServerConfig) -> Self {
        NodeServer {
            config,
            is_running: false,
        }
    }

    pub fn config(&self) -> &NodeServerConfig {
        &self.config
    }

    pub fn is_running(&self) -> bool {
        self.is_running
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = NodeServerConfig::default();
        assert_eq!(config.listen_addr.port(), 3001);
        assert_eq!(config.max_connections, 200);
    }

    #[test]
    fn test_server_new() {
        let server = NodeServer::new(NodeServerConfig::default());
        assert!(!server.is_running());
    }
}
