use std::pin::Pin;

use anyhow::{bail, Result};

use sdk::{
    info, Blob, BlobData, BlobIndex, ContractAction, ContractInput, ContractName, HyleOutput,
    Identity, StateDigest,
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
    fn get_state(&self, contract_name: &ContractName) -> Result<StateDigest>;
    fn get_onchain_state(&self, contract_name: &ContractName) -> Result<StateDigest> {
        self.get_state(contract_name)
    }
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
    pub fn add_action<CF: ContractAction>(
        &mut self,
        contract_name: ContractName,
        binary: &'static [u8],
        action: CF,
        caller: Option<BlobIndex>,
        callees: Option<Vec<BlobIndex>>,
    ) -> Result<&'_ mut ContractRunner> {
        let runner = ContractRunner::new(
            contract_name.clone(),
            binary,
            self.identity.clone(),
            BlobIndex(self.blobs.len()),
        )?;
        self.runners.push(runner);
        self.blobs
            .push(action.as_blob(contract_name, caller, callees));
        Ok(self.runners.last_mut().unwrap())
    }

    pub fn build<S: StateUpdater>(&mut self, new_states: &mut S) -> Result<BuildResult> {
        let mut outputs = vec![];
        for runner in self.runners.iter_mut() {
            let state = new_states.get_state(&runner.contract_name)?;

            runner.contract_input.blobs = self.blobs.clone();
            runner.contract_input.private_blob = runner.private_blob(state.clone())?;
            runner.contract_input.initial_state =
                new_states.get_onchain_state(&runner.contract_name)?;

            let off_chain_new_state: Option<StateDigest> = runner.callback(state)?;
            let out = runner.execute()?;
            new_states.update(
                &runner.contract_name,
                off_chain_new_state
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
    offchain_cb: Option<Box<dyn Fn(StateDigest) -> Result<StateDigest> + Send + Sync>>,
    private_blob_cb: Option<Box<dyn Fn(StateDigest) -> Result<BlobData> + Send + Sync>>,
}

impl ContractRunner {
    fn new(
        contract_name: ContractName,
        binary: &'static [u8],
        identity: Identity,
        index: BlobIndex,
    ) -> Result<Self> {
        let contract_input = ContractInput {
            initial_state: StateDigest::default(),
            identity,
            tx_hash: "".into(),
            private_blob: BlobData::default(),
            blobs: vec![],
            index,
        };

        Ok(Self {
            contract_name,
            binary,
            contract_input,
            offchain_cb: None,
            private_blob_cb: None,
        })
    }

    pub fn build_offchain_state<F>(&mut self, f: F) -> &mut Self
    where
        F: Fn(StateDigest) -> Result<StateDigest> + Send + Sync + 'static,
    {
        self.offchain_cb = Some(Box::new(f));
        self
    }

    pub fn with_private_blob<F>(&mut self, f: F) -> &mut Self
    where
        F: Fn(StateDigest) -> Result<BlobData> + Send + Sync + 'static,
    {
        self.private_blob_cb = Some(Box::new(f));
        self
    }

    fn callback(&self, state: StateDigest) -> Result<Option<StateDigest>> {
        self.offchain_cb
            .as_ref()
            .map(|cb| cb(state))
            .map_or(Ok(None), |v| v.map(Some))
    }

    fn private_blob(&self, state: StateDigest) -> Result<BlobData> {
        self.private_blob_cb
            .as_ref()
            .map(|cb| cb(state))
            .map_or(Ok(BlobData::default()), |v| v)
    }

    fn execute(&self) -> Result<HyleOutput> {
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

    async fn prove(&self) -> Result<ProofData> {
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
