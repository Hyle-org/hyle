use anyhow::Context;
use assert_cmd::prelude::*;
use hyle::utils::conf::Conf;
use signal_child::signal;
use tempfile::TempDir;
use tokio::io::AsyncBufReadExt;
use tokio::process::{Child, Command};
use tracing::info;

pub struct TestProcess {
    pub conf: Conf,
    #[allow(dead_code)]
    pub dir: TempDir,

    cmd: Command,
    process: Option<Child>,

    stdout: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
    stderr: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
}

async fn stream_output<R: tokio::io::AsyncRead + Unpin>(
    output: R,
    nid: String,
) -> anyhow::Result<()> {
    let mut reader = tokio::io::BufReader::new(output).lines();
    while let Some(line) = reader.next_line().await? {
        // Hack because the r0vm child process prints to our own stdout
        if line.starts_with("R0VM") {
            tracing::debug!("{} {}", nid, line);
        } else {
            println!("{}", line);
        }
    }
    Ok(())
}
impl TestProcess {
    pub fn new(command: &str, mut conf: Conf) -> Self {
        info!("🚀 Starting process {}", conf.id);
        tracing::debug!("Configuration: {:?}", conf);
        let mut cargo_bin: Command = std::process::Command::cargo_bin(command).unwrap().into();

        // Create a temporary directory for the node
        let tmpdir = tempfile::Builder::new().prefix("hyle").tempdir().unwrap();
        let cmd = cargo_bin.current_dir(&tmpdir);
        cmd.kill_on_drop(true);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        conf.data_directory = tmpdir.path().to_path_buf();
        // Serialize the configuration to a file
        let conf_file = tmpdir.path().join("config.ron");
        ron::ser::to_writer(std::fs::File::create(&conf_file).unwrap(), &conf).unwrap();

        cmd.env("RISC0_DEV_MODE", "1");
        Self {
            conf,
            dir: tmpdir,
            cmd: cargo_bin,
            process: None,
            stdout: None,
            stderr: None,
        }
    }

    #[allow(dead_code)]
    pub fn log(mut self, level: &str) -> Self {
        self.cmd.env("RUST_LOG", level);
        self
    }

    pub fn start(mut self) -> Self {
        if let Some(process) = &self.process {
            panic!("Process already started: {:?}", process.id());
        }
        tracing::debug!("Starting process: {:?}", self.cmd);
        self.process = Some({
            let mut process = self.cmd.spawn().unwrap();
            let stdout = process.stdout.take().expect("Failed to capture stdout");
            let stderr = process.stderr.take().expect("Failed to capture stderr");

            self.stdout = Some(tokio::task::spawn(stream_output(
                stdout,
                self.conf.id.clone(),
            )));
            self.stderr = Some(tokio::task::spawn(stream_output(
                stderr,
                self.conf.id.clone(),
            )));

            process
        });

        self
    }

    pub async fn stop(&mut self) -> anyhow::Result<()> {
        if let Some(mut process) = self.process.take() {
            // TODO: support windows?
            signal(process.id().unwrap().try_into().unwrap(), signal::SIGQUIT)
                .context("Failed to stop child")?;
            process.wait().await.context("Failed to wait for process")?;
            Ok(())
        } else {
            anyhow::bail!("Process not started")
        }
    }
}
