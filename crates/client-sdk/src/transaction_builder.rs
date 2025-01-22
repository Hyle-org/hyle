use std::{
    collections::BTreeMap,
    future::Future,
    ops::{Deref, DerefMut},
    sync::{Arc, OnceLock},
};

use anyhow::{bail, Result};
use sdk::{
    info, Blob, BlobData, BlobIndex, BlobTransaction, ContractAction, ContractInput, ContractName,
    Hashable, HyleOutput, Identity, ProofData, StateDigest,
};

use crate::helpers::{ClientSdkExecutor, ClientSdkProver};

pub struct ProvableBlobTx {
    pub identity: Identity,
    pub blobs: Vec<Blob>,
    runners: Vec<ContractRunner>,
}

impl ProvableBlobTx {
    pub fn new(identity: Identity) -> Self {
        ProvableBlobTx {
            identity,
            runners: vec![],
            blobs: vec![],
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_action<CF: ContractAction>(
        &mut self,
        contract_name: ContractName,
        action: CF,
        caller: Option<BlobIndex>,
        callees: Option<Vec<BlobIndex>>,
    ) -> Result<&'_ mut ContractRunner> {
        let runner = ContractRunner::new(
            contract_name.clone(),
            self.identity.clone(),
            BlobIndex(self.blobs.len()),
        )?;
        self.runners.push(runner);
        self.blobs
            .push(action.as_blob(contract_name, caller, callees));
        Ok(self.runners.last_mut().unwrap())
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
    ) -> impl Iterator<Item = (impl Future<Output = Result<ProofData>> + Send, ContractName)> {
        self.runners.into_iter().map(move |mut runner| {
            info!("Proving transition for {}...", runner.contract_name);
            let prover = self
                .provers
                .get(&runner.contract_name)
                .expect("no prover defined")
                .clone();
            let future = async move {
                prover
                    .prove(runner.contract_input.take().expect("no input for prover"))
                    .await
            };
            (future, runner.contract_name.clone())
        })
    }
}

pub trait StateUpdater
where
    Self: std::marker::Sized,
{
    fn setup(&self, ctx: &mut TxExecutorBuilder<Self>);
    fn update(&mut self, contract_name: &ContractName, new_state: StateDigest) -> Result<()>;
    fn get(&self, contract_name: &ContractName) -> Result<StateDigest>;
}

#[derive(Default)]
pub struct TxExecutor<S: StateUpdater> {
    full_states: S,
    on_chain_states: BTreeMap<ContractName, StateDigest>,
    executors: BTreeMap<ContractName, Box<dyn ClientSdkExecutor + Sync + Send>>,
    provers: BTreeMap<ContractName, Arc<dyn ClientSdkProver + Sync + Send>>,
}

impl<S: StateUpdater> Deref for TxExecutor<S> {
    type Target = S;

    fn deref(&self) -> &Self::Target {
        &self.full_states
    }
}
impl<S: StateUpdater> DerefMut for TxExecutor<S> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.full_states
    }
}

pub struct TxExecutorBuilder<S> {
    full_states: Option<S>,
    on_chain_states: BTreeMap<ContractName, StateDigest>,
    executors: BTreeMap<ContractName, Box<dyn ClientSdkExecutor + Sync + Send>>,
    provers: BTreeMap<ContractName, Arc<dyn ClientSdkProver + Sync + Send>>,
}

impl<S: StateUpdater> TxExecutorBuilder<S> {
    pub fn new(full_states: S) -> Self {
        let mut ret = Self {
            full_states: None,
            on_chain_states: BTreeMap::new(),
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
            full_states: self.full_states.unwrap(),
            on_chain_states: self.on_chain_states,
            executors: self.executors,
            provers: self.provers,
        }
    }

    pub fn init_with(
        &mut self,
        contract_name: ContractName,
        state: StateDigest,
        executor: impl ClientSdkExecutor + Sync + Send + 'static,
        prover: impl ClientSdkProver + Sync + Send + 'static,
    ) -> &mut Self {
        self.on_chain_states
            .entry(contract_name.clone())
            .or_insert(state);
        self.executors
            .entry(contract_name.clone())
            .or_insert(Box::new(executor));
        self.provers
            .entry(contract_name)
            .or_insert(Arc::new(prover));
        self
    }

    pub fn with_onchain_state(mut self, contract_name: ContractName, state: StateDigest) -> Self {
        self.on_chain_states.insert(contract_name, state);
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
            let on_chain_state = self
                .on_chain_states
                .get(&runner.contract_name)
                .cloned()
                .ok_or(anyhow::anyhow!("State not found"))?;
            let full_state = self.full_states.get(&runner.contract_name)?.clone();

            let private_blob = runner.private_blob(full_state.clone())?;

            runner.build_input(tx.blobs.clone(), private_blob, on_chain_state.clone());

            info!("Checking transition for {}...", runner.contract_name);
            let out = self
                .executors
                .get(&runner.contract_name)
                .unwrap()
                .execute(runner.contract_input.get().unwrap())?;

            if !out.success {
                let program_error = std::str::from_utf8(&out.program_outputs).unwrap();
                bail!("Execution failed ! Program output: {}", program_error);
            }

            self.on_chain_states
                .entry(runner.contract_name.clone())
                .and_modify(|v| *v = out.next_state.clone());

            if let Some(off_chain_new_state) = runner.callback(full_state)? {
                self.full_states
                    .update(&runner.contract_name, off_chain_new_state)?;
            } else {
                self.full_states
                    .update(&runner.contract_name, out.next_state.clone())?;
            }

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
    contract_input: OnceLock<ContractInput>,
    offchain_cb: Option<Box<dyn Fn(StateDigest) -> Result<StateDigest> + Send + Sync>>,
    private_blob_cb: Option<Box<dyn Fn(StateDigest) -> Result<BlobData> + Send + Sync>>,
}

impl ContractRunner {
    fn new(contract_name: ContractName, identity: Identity, index: BlobIndex) -> Result<Self> {
        Ok(Self {
            contract_name,
            identity,
            index,
            contract_input: OnceLock::new(),
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

    fn build_input(
        &mut self,
        blobs: Vec<Blob>,
        private_blob: BlobData,
        initial_state: StateDigest,
    ) {
        let tx_hash = BlobTransaction {
            identity: self.identity.clone(),
            blobs: blobs.clone(),
        }
        .hash();

        self.contract_input.get_or_init(|| ContractInput {
            identity: self.identity.clone(),
            tx_hash,
            blobs,
            private_blob,
            index: self.index.clone(),
            initial_state,
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
                new_state: StateDigest,
            ) -> anyhow::Result<()> {
                match contract_name.0.as_str() {
                    $(stringify!($contract_name) => self.$contract_name = new_state.try_into()?,)*
                    _ => anyhow::bail!("Unknown contract name: {contract_name}"),
                };
                Ok(())
            }

            fn get(&self, contract_name: &ContractName) -> anyhow::Result<StateDigest> {
                match contract_name.0.as_str() {
                    $(stringify!($contract_name) => Ok(Digestable::as_digest(&self.$contract_name)),)*
                    _ => anyhow::bail!("Unknown contract name: {contract_name}"),
                }
            }
        }
    };
}
