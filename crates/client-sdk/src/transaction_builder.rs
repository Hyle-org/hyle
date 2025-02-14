use std::{
    any::Any,
    collections::BTreeMap,
    future::Future,
    ops::{Deref, DerefMut},
    sync::{Arc, OnceLock},
};

use anyhow::{bail, Result};
use sdk::{
    Blob, BlobIndex, BlobTransaction, ContractAction, ContractInput, ContractName, Digestable,
    Hashable, HyleOutput, Identity, ProofTransaction, TxContext,
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

impl From<ProvableBlobTx> for BlobTransaction {
    fn from(tx: ProvableBlobTx) -> Self {
        BlobTransaction {
            identity: tx.identity,
            blobs: tx.blobs,
        }
    }
}

pub struct ProofTxBuilder {
    pub identity: Identity,
    pub blobs: Vec<Blob>,
    runners: Vec<ContractRunner>,
    pub outputs: Vec<(ContractName, HyleOutput)>,
    provers: BTreeMap<ContractName, Arc<dyn ClientSdkProver<Box<dyn Any>> + Sync + Send>>,
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
    ) -> impl Iterator<Item = impl Future<Output = Result<ProofTransaction>>> {
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

pub trait StateTrait: Digestable + 'static {}

pub trait StateUpdater
where
    Self: std::marker::Sized,
{
    fn setup(&self, ctx: &mut TxExecutorBuilder<Self>);
    fn update(&mut self, contract_name: &ContractName, new_state: &mut dyn Any) -> Result<()>;
    fn get(&self, contract_name: &ContractName) -> Result<Box<dyn Any + Send + 'static>>;
}

pub struct TxExecutor<S: StateUpdater> {
    states: S,
    executors: BTreeMap<ContractName, Box<dyn ClientSdkExecutor>>,
    provers: BTreeMap<ContractName, Arc<dyn ClientSdkProver<Box<dyn Any>> + Sync + Send>>,
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
    states: Option<S>,
    executors: BTreeMap<ContractName, Box<dyn ClientSdkExecutor>>,
    provers: BTreeMap<ContractName, Arc<dyn ClientSdkProver<Box<dyn Any>> + Sync + Send>>,
}

impl<S: StateUpdater> TxExecutorBuilder<S> {
    pub fn new(states: S) -> Self {
        let mut ret = Self {
            states: None,
            executors: BTreeMap::new(),
            provers: BTreeMap::new(),
        };
        states.setup(&mut ret);
        ret.states = Some(states);
        ret
    }

    pub fn with_contract(
        &mut self,
        contract_name: ContractName,
        executor: impl ClientSdkExecutor + Sync + Send + 'static,
        prover: impl ClientSdkProver<Box<dyn Any>> + Sync + Send + 'static,
    ) -> &mut Self {
        self.executors
            .insert(contract_name.clone(), Box::new(executor));
        self.provers.insert(contract_name, Arc::new(prover));
        self
    }

    pub fn with_executor(
        mut self,
        contract_name: ContractName,
        executor: Box<dyn ClientSdkExecutor>,
    ) -> Self {
        self.executors.insert(contract_name, executor);
        self
    }

    pub fn with_prover(
        mut self,
        contract_name: ContractName,
        prover: Arc<dyn ClientSdkProver<Box<dyn Any>> + Sync + Send + 'static>,
    ) -> Self {
        self.provers.insert(contract_name, prover);
        self
    }

    pub fn build(self) -> TxExecutor<S> {
        TxExecutor {
            // Safe to unwrap because we set it in the constructor
            states: self.states.unwrap(),
            executors: self.executors,
            provers: self.provers,
        }
    }
}

impl<S: StateUpdater> TxExecutor<S> {
    pub fn process_all<'a, I>(
        &'a mut self,
        iter: I,
    ) -> impl Iterator<Item = Result<ProofTxBuilder>> + 'a
    where
        I: IntoIterator<Item = ProvableBlobTx> + 'a,
    {
        iter.into_iter().map(move |tx| self.process(tx))
    }

    pub fn process(&mut self, mut tx: ProvableBlobTx) -> Result<ProofTxBuilder> {
        let mut outputs = vec![];
        for runner in tx.runners.iter_mut() {
            let state = self.states.get(&runner.contract_name)?;

            runner.build_input(tx.tx_context.clone(), tx.blobs.clone(), state);

            tracing::info!("Checking transition for {}...", runner.contract_name);
            let (mut next_state, out) = {
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

            self.states
                .update(&runner.contract_name, &mut *next_state)?;

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

pub struct ContractRunner {
    pub contract_name: ContractName,
    identity: Identity,
    index: BlobIndex,
    contract_input: OnceLock<ContractInput<Box<dyn Any + Send + 'static>>>,
    private_input: Option<Vec<u8>>,
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
            contract_input: OnceLock::new(),
            private_input,
        })
    }

    fn build_input(
        &mut self,
        tx_context: Option<TxContext>,
        blobs: Vec<Blob>,
        initial_state: Box<dyn Any + Send + 'static>,
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

            fn get(&self, contract_name: &ContractName) -> anyhow::Result<Box<dyn std::any::Any + Send + 'static>> {
                match contract_name.0.as_str() {
                    $(stringify!($contract_name) => Ok(Box::new(self.$contract_name.clone())),)*
                    _ => anyhow::bail!("Unknown contract name: {contract_name}"),
                }
            }
        }
    };
}
