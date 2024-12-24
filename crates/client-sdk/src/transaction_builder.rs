use std::pin::Pin;

use anyhow::{bail, Result};
use serde::Serialize;

use sdk::{
    info, Blob, BlobData, BlobIndex, ContractAction, ContractInput, ContractName, Digestable,
    HyleOutput, Identity, StateDigest,
};

use crate::ProofData;

// TO be implemented by each contract
pub struct TxBuilder<'a, 'b, State> {
    pub state: &'a State,
    pub contract_name: ContractName,
    pub builder: &'b mut TransactionBuilder,
}

pub struct BuildResult {
    pub identity: Identity,
    pub blobs: Vec<Blob>,
    pub outputs: Vec<(ContractName, HyleOutput)>,
}

pub struct TransactionBuilder {
    pub identity: Identity,
    runners: Vec<ContractRunner>,
    pub blobs: Vec<Blob>,
}

pub trait StateUpdater {
    fn update(&mut self, contract_name: &ContractName, new_state: StateDigest) -> Result<()>;
}

impl TransactionBuilder {
    pub fn new(identity: Identity) -> Self {
        TransactionBuilder {
            identity,
            runners: vec![],
            blobs: vec![],
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_action<CF: ContractAction, State: Digestable + Serialize>(
        &mut self,
        contract_name: ContractName,
        binary: &'static [u8],
        initial_state: State,
        action: CF,
        private_blob: BlobData,
        caller: Option<BlobIndex>,
        callees: Option<Vec<BlobIndex>>,
        off_chain_new_state: Option<StateDigest>,
    ) -> Result<()> {
        let runner = ContractRunner::new(
            contract_name.clone(),
            binary,
            self.identity.clone(),
            private_blob,
            BlobIndex(self.blobs.len()),
            initial_state.as_digest(),
            off_chain_new_state,
        )?;
        self.runners.push(runner);
        self.blobs
            .push(action.as_blob(contract_name, caller, callees));
        Ok(())
    }

    pub fn build<S: StateUpdater>(&mut self, new_states: &mut S) -> Result<BuildResult> {
        let mut outputs = vec![];
        for runner in self.runners.iter_mut() {
            runner.set_blobs(self.blobs.clone());
            let out = runner.execute()?;
            new_states.update(
                &runner.contract_name,
                runner
                    .off_chain_new_state
                    .clone()
                    .unwrap_or(out.next_state.clone()),
            )?;
            outputs.push((runner.contract_name.clone(), out));
        }

        Ok(BuildResult {
            identity: self.identity.clone(),
            blobs: self.blobs.clone(),
            outputs,
        })
    }

    /// Returns an iterator over the proofs of the transactions
    /// In order to send proofs when they are ready, without waiting for all of them to be ready
    /// Example usage:
    /// for (proof, contract_name) in transaction.iter_prove() {
    ///    let proof: ProofData = proof.await.unwrap();
    ///    ctx.client()
    ///        .send_tx_proof(&hyle::model::ProofTransaction {
    ///            blob_tx_hash: blob_tx_hash.clone(),
    ///            proof,
    ///            contract_name,
    ///        })
    ///        .await
    ///        .unwrap();
    ///}
    pub fn iter_prove<'a>(
        &'a self,
    ) -> impl Iterator<
        Item = (
            Pin<Box<dyn std::future::Future<Output = Result<ProofData>> + Send + 'a>>,
            ContractName,
        ),
    > + 'a {
        self.runners.iter().map(|runner| {
            let future = runner.prove();
            (
                Box::pin(future)
                    as Pin<Box<dyn std::future::Future<Output = Result<ProofData>> + Send + 'a>>,
                runner.contract_name.clone(),
            )
        })
    }
}

pub struct ContractRunner {
    pub contract_name: ContractName,
    binary: &'static [u8],
    contract_input: ContractInput,
    off_chain_new_state: Option<StateDigest>,
}

impl ContractRunner {
    pub fn new(
        contract_name: ContractName,
        binary: &'static [u8],
        identity: Identity,
        private_blob: BlobData,
        index: BlobIndex,
        initial_state: StateDigest,
        off_chain_new_state: Option<StateDigest>,
    ) -> Result<Self> {
        let contract_input = ContractInput {
            initial_state,
            identity,
            tx_hash: "".into(),
            private_blob,
            blobs: vec![],
            index,
        };

        Ok(Self {
            contract_name,
            binary,
            contract_input,
            off_chain_new_state,
        })
    }

    pub fn set_blobs(&mut self, blobs: Vec<Blob>) {
        self.contract_input.blobs = blobs;
    }

    pub fn execute(&self) -> Result<HyleOutput> {
        info!("Checking transition for {}...", self.contract_name);

        let contract_input = bonsai_runner::as_input_data(&self.contract_input)?;
        let execute_info = execute(self.binary, &contract_input)?;
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

        let contract_input = bonsai_runner::as_input_data(&self.contract_input)?;
        let explicit = std::env::var("RISC0_PROVER").unwrap_or_default();
        let receipt = match explicit.to_lowercase().as_str() {
            "bonsai" => bonsai_runner::run_bonsai(self.binary, contract_input.clone()).await?,
            _ => {
                let env = risc0_zkvm::ExecutorEnv::builder()
                    .write_slice(&contract_input)
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
}

fn execute(binary: &'static [u8], contract_input: &[u8]) -> Result<risc0_zkvm::SessionInfo> {
    let env = risc0_zkvm::ExecutorEnv::builder()
        .write_slice(contract_input)
        .build()
        .unwrap();

    let prover = risc0_zkvm::default_executor();
    prover.execute(env, binary)
}
