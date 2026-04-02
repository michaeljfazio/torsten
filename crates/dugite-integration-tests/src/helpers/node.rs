use crate::helpers::cli::{cli_path, node_path, run_cli};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Check if a node is running and responsive on the given socket.
pub fn is_node_running(socket: &str) -> bool {
    let result = run_cli(&["query", "tip", "--socket-path", socket]);
    result.success()
}

/// Handle to a running dugite-node process. Kills on drop.
pub struct NodeHandle {
    child: Option<Child>,
    pub socket_path: String,
}

impl NodeHandle {
    /// Start a dugite-node process.
    pub fn start(config: &str, topology: &str, database_path: &str, socket_path: &str) -> Self {
        let node = node_path();
        let child = Command::new(&node)
            .args([
                "run",
                "--config",
                config,
                "--topology",
                topology,
                "--database-path",
                database_path,
                "--socket-path",
                socket_path,
                "--host-addr",
                "0.0.0.0",
                "--port",
                "3001",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|e| panic!("Failed to start dugite-node at {:?}: {}", node, e));

        Self {
            child: Some(child),
            socket_path: socket_path.to_string(),
        }
    }
}

impl Drop for NodeHandle {
    fn drop(&mut self) {
        if let Some(ref mut child) = self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Wait for the node to become ready (query tip succeeds).
pub fn wait_for_ready(socket: &str, timeout_secs: u64) {
    let start = Instant::now();
    let timeout = Duration::from_secs(timeout_secs);
    let cli = cli_path();

    loop {
        if start.elapsed() > timeout {
            panic!(
                "Node at socket {} did not become ready within {}s",
                socket, timeout_secs
            );
        }

        let output = Command::new(&cli)
            .args(["query", "tip", "--socket-path", socket])
            .output();

        if let Ok(out) = output {
            if out.status.success() {
                return;
            }
        }

        thread::sleep(Duration::from_secs(2));
    }
}
