use std::{
    any::Any,
    collections::BTreeMap,
    future::Future,
    sync::{Arc, OnceLock},
};

use anyhow::{bail, Result};
use sdk::{
    Blob, BlobIndex, BlobTransaction, ContractAction, ContractInput, ContractName, Digestable,
    Hashable, HyleOutput, Identity, ProofTransaction, TxContext,
};

use crate::helpers::{ClientSdkExecutor, ClientSdkProver};

pub struct ProvableBlobTx<State: StateTrait> {
    pub identity: Identity,
    pub blobs: Vec<Blob>,
    runners: Vec<ContractRunner<State>>,
    tx_context: Option<TxContext>,
}

impl<State: StateTrait> ProvableBlobTx<State> {
    pub fn new(identity: Identity) -> ProvableBlobTx<State> {
        ProvableBlobTx::<State> {
            identity,
            runners: vec![],
            blobs: vec![],
            tx_context: None,
        }
    }

    pub fn add_action<CF: ContractAction>(
        &mut self,
        contract_name: ContractName,
        action: CF,
        private_input: Option<Vec<u8>>,
        caller: Option<BlobIndex>,
        callees: Option<Vec<BlobIndex>>,
    ) -> Result<()> {
        let runner = ContractRunner::new(
            contract_name.clone(),
            self.identity.clone(),
            BlobIndex(self.blobs.len()),
            private_input,
        )?;
        self.runners.push(runner);
        self.blobs
            .push(action.as_blob(contract_name, caller, callees));
        Ok(())
    }

    pub fn add_context(&mut self, tx_context: TxContext) {
        self.tx_context = Some(tx_context);
    }
}

impl<State: StateTrait> From<ProvableBlobTx<State>> for BlobTransaction {
    fn from(tx: ProvableBlobTx<State>) -> Self {
        BlobTransaction {
            identity: tx.identity,
            blobs: tx.blobs,
        }
    }
}

pub struct ProofTxBuilder<State: StateTrait> {
    pub identity: Identity,
    pub blobs: Vec<Blob>,
    runners: Vec<ContractRunner<State>>,
    pub outputs: Vec<(ContractName, HyleOutput)>,
    provers: BTreeMap<ContractName, Arc<dyn ClientSdkProver<State> + Sync + Send>>,
}

impl<State: StateTrait + Send> ProofTxBuilder<State> {
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
    pub fn iter_prove(
        self,
    ) -> impl Iterator<Item = impl Future<Output = Result<ProofTransaction>> + Send> {
        self.runners.into_iter().map(move |mut runner| {
            tracing::info!("Proving transition for {}...", runner.contract_name);
            let prover = self
                .provers
                .get(&runner.contract_name)
                .expect("no prover defined")
                .clone();
            async move {
                let proof = prover
                    .prove(runner.contract_input.take().expect("no input for prover"))
                    .await;
                proof.map(|proof| ProofTransaction {
                    proof,
                    contract_name: runner.contract_name.clone(),
                })
            }
        })
    }

    pub fn to_blob_tx(&self) -> BlobTransaction {
        BlobTransaction {
            identity: self.identity.clone(),
            blobs: self.blobs.clone(),
        }
    }
}

pub trait StateTrait: Digestable + 'static {
    fn as_any(&self) -> &dyn Any;
}

#[derive(Default)]
pub struct TxExecutorBuilder<State: StateTrait> {
    states: BTreeMap<ContractName, Box<dyn StateTrait>>,
    executors: BTreeMap<ContractName, Box<dyn ClientSdkExecutor<State> + Sync + Send>>,
    provers: BTreeMap<ContractName, Arc<dyn ClientSdkProver<State> + Sync + Send>>,
}

pub struct TxExecutor<State: StateTrait> {
    states: BTreeMap<ContractName, Box<dyn StateTrait>>,
    executors: BTreeMap<ContractName, Box<dyn ClientSdkExecutor<State> + Sync + Send>>,
    provers: BTreeMap<ContractName, Arc<dyn ClientSdkProver<State> + Sync + Send>>,
}

impl<State: StateTrait> TxExecutorBuilder<State> {
    pub fn with_contract(
        &mut self,
        contract_name: ContractName,
        state: Box<dyn StateTrait>,
        executor: Box<dyn ClientSdkExecutor<State> + Sync + Send>,
        prover: Arc<dyn ClientSdkProver<State> + Sync + Send>,
    ) -> &mut Self {
        self.states.insert(contract_name.clone(), state);
        self.executors.insert(contract_name.clone(), executor);
        self.provers.insert(contract_name, prover);
        self
    }

    pub fn build(self) -> TxExecutor<State> {
        TxExecutor {
            states: self.states,
            executors: self.executors,
            provers: self.provers,
        }
    }
}

impl<State: StateTrait> TxExecutor<State> {
    pub fn process_all<I>(
        &mut self,
        iter: I,
    ) -> impl Iterator<Item = Result<ProofTxBuilder<State>>> + use<'_, State, I>
    where
        I: IntoIterator<Item = ProvableBlobTx<State>>,
    {
        iter.into_iter().map(move |tx| self.process(tx))
    }

    pub fn process(&mut self, mut tx: ProvableBlobTx<State>) -> Result<ProofTxBuilder<State>> {
        let mut outputs = vec![];
        for runner in tx.runners.iter_mut() {
            let state = self
                .states
                .remove(&runner.contract_name)
                .ok_or(anyhow::anyhow!("State not found"))?;

            let state_any = state.as_any();
            let initial_state = state_any
                .downcast_ref::<State>()
                .ok_or(anyhow::anyhow!("Could not downcast state"))?;

            runner.build_input(tx.tx_context.clone(), tx.blobs.clone(), *initial_state);

            tracing::info!("Checking transition for {}...", runner.contract_name);
            let (next_state, out) = {
                let executor = self
                    .executors
                    .get(&runner.contract_name)
                    .ok_or(anyhow::anyhow!("Executor not found"))?
                    .as_ref();

                let contract_input = runner.contract_input.get().unwrap();

                executor.execute(contract_input)?
            };

            if !out.success {
                let program_error = std::str::from_utf8(&out.program_outputs).unwrap();
                bail!("Execution failed ! Program output: {}", program_error);
            }

            self.states.insert(
                runner.contract_name.clone(),
                next_state as Box<dyn StateTrait>,
            );

            outputs.push((runner.contract_name.clone(), out));
        }

        Ok(ProofTxBuilder {
            identity: tx.identity,
            blobs: tx.blobs,
            runners: tx.runners,
            outputs,
            provers: self.provers.clone(),
        })
    }
}

pub struct ContractRunner<State: StateTrait> {
    pub contract_name: ContractName,
    identity: Identity,
    index: BlobIndex,
    contract_input: OnceLock<ContractInput<State>>,
    private_input: Option<Vec<u8>>,
}

impl<State: StateTrait> ContractRunner<State> {
    fn new(
        contract_name: ContractName,
        identity: Identity,
        index: BlobIndex,
        private_input: Option<Vec<u8>>,
    ) -> Result<Self> {
        Ok(Self {
            contract_name,
            identity,
            index,
            contract_input: OnceLock::new(),
            private_input,
        })
    }

    fn build_input(
        &mut self,
        tx_context: Option<TxContext>,
        blobs: Vec<Blob>,
        initial_state: State,
    ) {
        let tx_hash = BlobTransaction {
            identity: self.identity.clone(),
            blobs: blobs.clone(),
        }
        .hash();

        self.contract_input.get_or_init(|| ContractInput {
            initial_state,
            identity: self.identity.clone(),
            index: self.index,
            blobs,
            tx_hash,
            tx_ctx: tx_context,
            private_input: self.private_input.clone().unwrap_or_default(),
        });
    }
}
