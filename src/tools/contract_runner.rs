use crate::{
    indexer::model::ContractDb,
    model::{ContractName, ProofData},
    rest::client::ApiHttpClient,
};
use anyhow::{bail, Error, Result};
use hyle_contract_sdk::{
    Blob, BlobData, BlobIndex, ContractInput, Digestable, HyleOutput, Identity, StateDigest,
};
use serde::Serialize;
use tracing::info;

pub struct ContractRunner {
    pub contract_name: ContractName,
    binary: &'static [u8],
    contract_input: Vec<u8>,
}

impl ContractRunner {
    pub async fn new<State>(
        contract_name: ContractName,
        binary: &'static [u8],
        identity: Identity,
        private_blob: BlobData,
        blobs: Vec<Blob>,
        index: BlobIndex,
        initial_state: State,
    ) -> Result<Self>
    where
        State: Digestable + Serialize,
    {
        let contract_input = ContractInput::<State> {
            initial_state,
            identity,
            tx_hash: "".into(),
            private_blob,
            blobs,
            index,
        };
        let contract_input = bonsai_runner::as_input_data(&contract_input)?;

        Ok(Self {
            contract_name,
            binary,
            contract_input,
        })
    }

    pub fn execute(&self) -> Result<HyleOutput> {
        info!("Checking transition for {}...", self.contract_name);

        let execute_info = execute(self.binary, &self.contract_input)?;
        let output = execute_info.journal.decode::<HyleOutput>().unwrap();
        if !output.success {
            let program_error = std::str::from_utf8(&output.program_outputs).unwrap();
            bail!(
                "\x1b[91mExecution failed ! Program output: {}\x1b[0m",
                program_error
            );
        }
        Ok(output)
    }

    pub async fn prove(&self) -> Result<ProofData> {
        info!("Proving transition for {}...", self.contract_name);

        let explicit = std::env::var("RISC0_PROVER").unwrap_or_default();
        let receipt = match explicit.to_lowercase().as_str() {
            "bonsai" => bonsai_runner::run_bonsai(self.binary, self.contract_input.clone()).await?,
            _ => {
                let env = risc0_zkvm::ExecutorEnv::builder()
                    .write_slice(&self.contract_input)
                    .build()
                    .unwrap();

                let prover = risc0_zkvm::default_prover();
                let prove_info = prover.prove(env, self.binary)?;
                prove_info.receipt
            }
        };

        let hyle_output = receipt
            .journal
            .decode::<HyleOutput>()
            .expect("Failed to decode journal");

        if !hyle_output.success {
            let program_error = std::str::from_utf8(&hyle_output.program_outputs).unwrap();
            bail!(
                "\x1b[91mExecution failed ! Program output: {}\x1b[0m",
                program_error
            );
        }

        let encoded_receipt = borsh::to_vec(&receipt).expect("Unable to encode receipt");
        Ok(ProofData::Bytes(encoded_receipt))
    }

    pub fn prove_test(hyle_output: HyleOutput) -> Result<ProofData> {
        Ok(ProofData::Bytes(serde_json::to_vec(&hyle_output)?))
    }
}

pub async fn fetch_current_state<State>(
    indexer_client: &ApiHttpClient,
    contract_name: &ContractName,
) -> Result<State, Error>
where
    State: TryFrom<hyle_contract_sdk::StateDigest, Error = Error>,
{
    let resp = indexer_client
        .get_indexer_contract(contract_name)
        .await?
        .json::<ContractDb>()
        .await?;

    StateDigest(resp.state_digest).try_into()
}

fn execute(binary: &'static [u8], contract_input: &[u8]) -> Result<risc0_zkvm::SessionInfo> {
    let env = risc0_zkvm::ExecutorEnv::builder()
        .write_slice(contract_input)
        .build()
        .unwrap();

    let prover = risc0_zkvm::default_executor();
    prover.execute(env, binary)
}
