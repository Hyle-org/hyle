use assert_cmd::prelude::*;
use hyle::utils::conf::{Conf, Consensus};
use std::process::{Child, Command};
use tempfile::TempDir;

pub struct ConfMaker {
    i: u16,
    pub default: Conf,
}

impl ConfMaker {
    pub fn build(&mut self) -> Conf {
        self.i += 1;
        Conf {
            id: format!("node-{}", self.i),
            host: format!("localhost:{}", 3000 + self.i),
            da_address: format!("localhost:{}", 4000 + self.i),
            rest: format!("localhost:{}", 5000 + self.i),
            ..self.default.clone()
        }
    }
}

impl Default for ConfMaker {
    fn default() -> Self {
        let mut default = Conf::new(None, None, None).unwrap();
        default.run_indexer = false; // disable indexer by default to avoid needed PG
        default.log_format = "node".to_string(); // Activate node name in logs for convenience in tests.
        Self { i: 0, default }
    }
}

enum TestProcessState {
    Command(Command),
    Child(Child),
}

pub struct TestProcess {
    pub conf: Conf,
    #[allow(dead_code)]
    pub dir: TempDir,
    state: TestProcessState,
}

impl TestProcess {
    pub fn new(command: &str, mut conf: Conf) -> Self {
        let mut cargo_bin = Command::cargo_bin(command).unwrap();

        // Create a temporary directory for the node
        let tmpdir = tempfile::Builder::new().prefix("hyle").tempdir().unwrap();
        let cmd = cargo_bin.current_dir(&tmpdir);

        conf.data_directory = tmpdir.path().to_path_buf();
        // Serialize the configuration to a file
        let conf_file = tmpdir.path().join("config.ron");
        ron::ser::to_writer(std::fs::File::create(&conf_file).unwrap(), &conf).unwrap();

        let console_port: u16 = conf.da_address.split(':').last().unwrap().parse().unwrap();
        cmd.env(
            "TOKIO_CONSOLE_BIND",
            format!("127.0.0.1:{}", console_port + 10000),
        );
        Self {
            conf,
            dir: tmpdir,
            state: TestProcessState::Command(cargo_bin),
        }
    }

    pub fn log(mut self, level: &str) -> Self {
        if let TestProcessState::Command(cmd) = &mut self.state {
            cmd.env("RUST_LOG", level);
        };
        self
    }

    pub fn start(mut self) -> Self {
        self.state = match &mut self.state {
            TestProcessState::Command(cmd) => {
                println!("Starting process: {:?}", cmd);
                TestProcessState::Child(cmd.spawn().unwrap())
            }
            TestProcessState::Child(child) => {
                panic!("Process already started: {:?}", child.id());
            }
        };
        self
    }
}

// Drop implem to be sure that process is well stopped
impl Drop for TestProcess {
    fn drop(&mut self) {
        match &mut self.state {
            TestProcessState::Command(_) => (),
            TestProcessState::Child(child) => {
                child.kill().unwrap();
                child.wait().unwrap();
            }
        }
    }
}
