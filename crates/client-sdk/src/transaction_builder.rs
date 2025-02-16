use std::{
    any::Any,
    collections::BTreeMap,
    future::Future,
    ops::{Deref, DerefMut},
    sync::{Arc, OnceLock},
};

use anyhow::{bail, Result};
use sdk::{
    Blob, BlobIndex, BlobTransaction, ContractAction, ContractInput, ContractName, Hashed,
    HyleOutput, Identity, ProgramInput, ProofTransaction, TxContext,
};

use crate::helpers::{ClientSdkExecutor, ClientSdkProver};

pub struct ProvableBlobTx {
    pub identity: Identity,
    pub blobs: Vec<Blob>,
    runners: Vec<ContractRunner>,
    tx_context: Option<TxContext>,
}

impl ProvableBlobTx {
    pub fn new(identity: Identity) -> Self {
        ProvableBlobTx {
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
    ) -> Result<&'_ mut ContractRunner> {
        let runner = ContractRunner::new(
            contract_name.clone(),
            self.identity.clone(),
            BlobIndex(self.blobs.len()),
            private_input,
        )?;
        self.runners.push(runner);
        self.blobs
            .push(action.as_blob(contract_name, caller, callees));
        Ok(self.runners.last_mut().unwrap())
    }

    pub fn add_context(&mut self, tx_context: TxContext) {
        self.tx_context = Some(tx_context);
    }
}

impl From<ProvableBlobTx> for BlobTransaction {
    fn from(tx: ProvableBlobTx) -> Self {
        BlobTransaction::new(tx.identity, tx.blobs)
    }
}

pub struct ProofTxBuilder {
    pub identity: Identity,
    pub blobs: Vec<Blob>,
    runners: Vec<ContractRunner>,
    pub outputs: Vec<(ContractName, HyleOutput)>,
    provers: BTreeMap<ContractName, Arc<dyn ClientSdkProver + Sync + Send>>,
}

impl ProofTxBuilder {
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
                    .prove(runner.program_input.take().expect("no input for prover"))
                    .await;
                proof.map(|proof| ProofTransaction {
                    proof,
                    contract_name: runner.contract_name.clone(),
                })
            }
        })
    }

    pub fn to_blob_tx(&self) -> BlobTransaction {
        BlobTransaction::new(self.identity.clone(), self.blobs.clone())
    }
}

pub trait StateUpdater
where
    Self: std::marker::Sized,
{
    fn setup(&self, ctx: &mut TxExecutorBuilder<Self>);
    fn update(&mut self, contract_name: &ContractName, new_state: &mut dyn Any) -> Result<()>;
    fn get(&self, contract_name: &ContractName) -> Result<Vec<u8>>;
}

#[derive(Default)]
pub struct TxExecutor<S: StateUpdater> {
    states: S,
    executors: BTreeMap<ContractName, Box<dyn ClientSdkExecutor + Sync + Send>>,
    provers: BTreeMap<ContractName, Arc<dyn ClientSdkProver + Sync + Send>>,
}

impl<S: StateUpdater> Deref for TxExecutor<S> {
    type Target = S;

    fn deref(&self) -> &Self::Target {
        &self.states
    }
}
impl<S: StateUpdater> DerefMut for TxExecutor<S> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.states
    }
}

pub struct TxExecutorBuilder<S> {
    full_states: Option<S>,
    executors: BTreeMap<ContractName, Box<dyn ClientSdkExecutor + Sync + Send>>,
    provers: BTreeMap<ContractName, Arc<dyn ClientSdkProver + Sync + Send>>,
}

impl<S: StateUpdater> TxExecutorBuilder<S> {
    pub fn new(full_states: S) -> Self {
        let mut ret = Self {
            full_states: None,
            executors: BTreeMap::new(),
            provers: BTreeMap::new(),
        };
        full_states.setup(&mut ret);
        ret.full_states = Some(full_states);
        ret
    }

    pub fn build(self) -> TxExecutor<S> {
        TxExecutor {
            // Safe to unwrap because we set it in the constructor
            states: self.full_states.unwrap(),
            executors: self.executors,
            provers: self.provers,
        }
    }

    pub fn init_with(
        &mut self,
        contract_name: ContractName,
        executor: impl ClientSdkExecutor + Sync + Send + 'static,
        prover: impl ClientSdkProver + Sync + Send + 'static,
    ) -> &mut Self {
        self.executors
            .entry(contract_name.clone())
            .or_insert(Box::new(executor));
        self.provers
            .entry(contract_name)
            .or_insert(Arc::new(prover));
        self
    }

    pub fn with_executor(
        mut self,
        contract_name: ContractName,
        executor: impl ClientSdkExecutor + Sync + Send + 'static,
    ) -> Self {
        self.executors.insert(contract_name, Box::new(executor));
        self
    }

    pub fn with_prover(
        mut self,
        contract_name: ContractName,
        prover: impl ClientSdkProver + Sync + Send + 'static,
    ) -> Self {
        self.provers.insert(contract_name, Arc::new(prover));
        self
    }
}

impl<S: StateUpdater> TxExecutor<S> {
    pub fn process_all<I>(
        &mut self,
        iter: I,
    ) -> impl Iterator<Item = Result<ProofTxBuilder>> + use<'_, S, I>
    where
        I: IntoIterator<Item = ProvableBlobTx>,
    {
        iter.into_iter().map(move |tx| self.process(tx))
    }

    pub fn process(&mut self, mut tx: ProvableBlobTx) -> Result<ProofTxBuilder> {
        let mut outputs = vec![];
        for runner in tx.runners.iter_mut() {
            let serialized_initial_state = self.states.get(&runner.contract_name)?;

            runner.build_program_input(
                tx.tx_context.clone(),
                tx.blobs.clone(),
                serialized_initial_state,
            );

            tracing::info!("Checking transition for {}...", runner.contract_name);
            let (mut state, out) = self
                .executors
                .get(&runner.contract_name)
                .unwrap()
                .execute(runner.program_input.get().unwrap())?;

            if !out.success {
                let program_error = std::str::from_utf8(&out.program_outputs).unwrap();
                bail!("Execution failed ! Program output: {}", program_error);
            }

            self.states.update(&runner.contract_name, &mut *state)?;

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

#[derive(Debug)]
pub struct ContractRunner {
    pub contract_name: ContractName,
    identity: Identity,
    index: BlobIndex,
    private_input: Option<Vec<u8>>,
    program_input: OnceLock<ProgramInput>,
}

impl ContractRunner {
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
            private_input,
            program_input: OnceLock::new(),
        })
    }

    fn build_program_input(
        &mut self,
        tx_context: Option<TxContext>,
        blobs: Vec<Blob>,
        serialized_initial_state: Vec<u8>,
    ) {
        let tx_hash = BlobTransaction::new(self.identity.clone(), blobs.clone()).hashed();

        self.program_input.get_or_init(|| ProgramInput {
            serialized_initial_state,
            contract_input: ContractInput {
                identity: self.identity.clone(),
                index: self.index,
                blobs,
                tx_hash,
                tx_ctx: tx_context,
                private_input: self.private_input.clone().unwrap_or_default(),
            },
        });
    }
}

/// Macro to easily define the full state of a TxExecutor
/// Struct-like syntax.
/// Must have ContractName, StateDigest, Digestable and anyhow in scope.
#[macro_export]
macro_rules! contract_states {
    ($(#[$meta:meta])* $vis:vis struct $name:ident { $($mvis:vis $contract_name:ident: $contract_type:ty,)* }) => {
        $(#[$meta])*
        $vis struct $name {
            $($mvis $contract_name: $contract_type,
            )*
        }

        impl $crate::transaction_builder::StateUpdater for $name {
            fn setup(&self, ctx: &mut TxExecutorBuilder<Self>) {
                $(self.$contract_name.setup_builder::<Self>(stringify!($contract_name).into(), ctx);)*
            }

            fn update(
                &mut self,
                contract_name: &ContractName,
                new_state: &mut dyn std::any::Any,
            ) -> anyhow::Result<()> {
                match contract_name.0.as_str() {
                    $(stringify!($contract_name) => {
                        let Some(st) = new_state.downcast_mut::<$contract_type>() else {
                            anyhow::bail!("Incorrect state data passed for contract '{}'", contract_name);
                        };
                        std::mem::swap(&mut self.$contract_name, st);
                    })*
                    _ => anyhow::bail!("Unknown contract name: {contract_name}"),
                };
                Ok(())
            }

            fn get(&self, contract_name: &ContractName) -> anyhow::Result<Vec<u8>> {
                match contract_name.0.as_str() {
                    $(stringify!($contract_name) => Ok(borsh::to_vec(&self.$contract_name).map_err(|e| anyhow::anyhow!(e))?),)*
                    _ => anyhow::bail!("Unknown contract name: {contract_name}"),
                }
            }
        }
    };
}
