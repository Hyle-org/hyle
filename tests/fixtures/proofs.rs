#![allow(unused)]

use super::ctx::E2ECtx;
use hyrun::{Cli, CliCommand};

const AMM_IMG: &[u8] = include_bytes!("../../contracts/amm/amm.img");
const HYDENTITY_IMG: &[u8] = include_bytes!("../../contracts/hydentity/hydentity.img");
const HYLLAR_IMG: &[u8] = include_bytes!("../../contracts/hyllar/hyllar.img");

pub struct HyrunProofGen {
    dir: tempfile::TempDir,
}

impl HyrunProofGen {
    pub fn setup_working_directory() -> Self {
        // Setup: move to a temp directory, setup contracts, set env var.
        let tempdir = tempfile::tempdir().unwrap();

        std::env::set_var("RISC0_DEV_MODE", "1");

        // Write our binary contract there
        // Make sure to create directory if it doesn't exist
        std::fs::create_dir_all(tempdir.path().join("contracts/hydentity")).unwrap();
        std::fs::create_dir_all(tempdir.path().join("contracts/amm")).unwrap();
        std::fs::create_dir_all(tempdir.path().join("contracts/hyllar")).unwrap();
        std::fs::write(
            tempdir.path().join("contracts/hydentity/hydentity.img"),
            HYDENTITY_IMG,
        )
        .unwrap();
        std::fs::write(tempdir.path().join("contracts/amm/amm.img"), AMM_IMG).unwrap();
        std::fs::write(
            tempdir.path().join("contracts/hyllar/hyllar.img"),
            HYLLAR_IMG,
        )
        .unwrap();
        Self { dir: tempdir }
    }

    // Proofs are generated as [blob_index].risc0.proof
    pub async fn generate_proof(
        &self,
        ctx: &E2ECtx,
        command: CliCommand,
        user: &'static str,
        password: &'static str,
        nonce: Option<u32>,
    ) {
        let host = ctx.client().url.host().unwrap().to_string();
        let port = ctx.client().url.port().unwrap().into();
        let path_prefix = self.dir.path().to_str().unwrap().to_owned() + "/";
        let _ = tokio::task::spawn_blocking(move || {
            hyrun::run_command(Cli {
                command,
                user: Some(user.to_owned()),
                password: Some(password.to_owned()),
                nonce,
                host,
                port,
                path_prefix,
            });
        })
        .await;
    }

    pub fn read_proof(&self, blob_index: u32) -> Vec<u8> {
        std::fs::read(self.dir.path().join(format!("{}.risc0.proof", blob_index))).unwrap()
    }
}
