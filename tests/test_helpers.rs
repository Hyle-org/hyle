use assert_cmd::prelude::*;
use hyle::utils::conf::Conf;
use std::process::{Child, Command};
use tempfile::TempDir;

#[derive(Default)]
pub struct ConfMaker(u16);

impl ConfMaker {
    pub fn build(&mut self) -> Conf {
        let defaults = Conf::new(None, None, None).unwrap();
        self.0 += 1;
        Conf {
            id: format!("node-{}", self.0),
            host: format!("localhost:{}", 3000 + self.0),
            da_address: format!("localhost:{}", 4000 + self.0),
            rest: format!("localhost:{}", 5000 + self.0),
            run_indexer: false, // disable indexer by default to avoid needed PG
            ..defaults.clone()
        }
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
