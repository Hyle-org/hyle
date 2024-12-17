#![allow(unused)]

use std::collections::HashMap;

use super::ctx::E2ECtx;
use hyrun::{Cli, CliCommand, Context, ContractData};
use risc0_recursion::ProofInput;

pub struct HyrunProofGen {
    dir: tempfile::TempDir,
}

impl HyrunProofGen {
    pub fn setup_working_directory() -> Self {
        // Setup: move to a temp directory, setup contracts, set env var.
        let mut tempdir = tempfile::tempdir().unwrap();

        std::env::set_var("RISC0_DEV_MODE", "1");

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
        let proof_path = self.dir.path().to_str().unwrap().to_owned() + "/";
        let _ = tokio::task::spawn_blocking(move || {
            hyrun::run_command(&Context {
                cli: Cli {
                    command,
                    user: Some(user.to_owned()),
                    password: Some(password.to_owned()),
                    nonce,
                    host,
                    port,
                    proof_path,
                },
                contract_data: ContractData {
                    amm_elf: hyle_contracts::AMM_ELF.to_vec(),
                    amm_id: hyle_contracts::AMM_ID.to_vec(),
                    hyllar_elf: hyle_contracts::HYLLAR_ELF.to_vec(),
                    hyllar_id: hyle_contracts::HYLLAR_ID.to_vec(),
                    hydentity_elf: hyle_contracts::HYDENTITY_ELF.to_vec(),
                    hydentity_id: hyle_contracts::HYDENTITY_ID.to_vec(),
                },
                hardcoded_initial_states: HashMap::new(),
            });
        })
        .await;
    }

    pub async fn generate_recursive_proof(
        &self,
        program_ids: &[[u8; 32]],
        proofs: &[&[u8]],
    ) -> Vec<u8> {
        let receipts = proofs
            .iter()
            .map(|proof| {
                borsh::from_slice::<risc0_zkvm::Receipt>(proof).expect("Failed to decode receipt")
            })
            .collect::<Vec<_>>();

        let mut env = risc0_zkvm::ExecutorEnv::builder();
        receipts.iter().for_each(|receipt| {
            env.add_assumption(receipt.clone());
        });
        env.write(
            &std::iter::zip(program_ids, receipts)
                .map(|(pid, r)| ProofInput {
                    image_id: *pid,
                    journal: r.journal.bytes,
                })
                .collect::<Vec<ProofInput>>(),
        )
        .unwrap();
        let env = env.build().unwrap();

        let receipt = risc0_zkvm::default_prover()
            .prove(env, hyle_contracts::RISC0_RECURSION_ELF)
            .unwrap()
            .receipt;

        borsh::to_vec(&receipt).unwrap()
    }

    pub fn read_proof(&self, blob_index: u32) -> Vec<u8> {
        std::fs::read(self.dir.path().join(format!("{}.risc0.proof", blob_index))).unwrap()
    }
}
