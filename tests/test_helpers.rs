use assert_cmd::prelude::*;
use std::process::{Child, Command};

pub struct TestNode {
    child: Child,
}

impl TestNode {
    // Create a new process that spins up a node or a client
    pub fn new(config_file: &str, is_client: bool) -> Self {
        let mut cargo_bin = Command::cargo_bin(if is_client { "client" } else { "node" }).unwrap();
        let cmd = cargo_bin.arg("--config-file").arg(config_file);
        cmd.arg("--id").arg("0");

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
