use assert_cmd::prelude::*;
use std::{
    path::Path,
    process::{Child, Command},
};

pub struct TestNode {
    child: Child,
}

impl TestNode {
    // Create a new process that spins up a node or a client
    pub fn new(config_path: &Path, is_client: bool, console_bind_port: &str) -> Self {
        let mut cargo_bin = Command::cargo_bin(if is_client { "client" } else { "node" }).unwrap();
        let mut cmd = cargo_bin
            .arg("--config-file")
            .arg("conf.ron")
            .current_dir(config_path);
        if is_client {
            cmd = cmd.arg("send").arg("blob").arg("data/tx1_blob.ron")
        }

        // When spinning up multiple node, they need to use different ports for tracing
        cmd.env(
            "TOKIO_CONSOLE_BIND",
            format!("127.0.0.1:{}", console_bind_port),
        );
        let child = cmd.spawn().expect("Failed to start node");
        TestNode { child }
    }
}

// Drop implem to be sure that process is well stopped
impl Drop for TestNode {
    fn drop(&mut self) {
        let _ = self.child.kill(); // Kill child process if still active
        let _ = self.child.wait(); // Wait for end of process
    }
}
