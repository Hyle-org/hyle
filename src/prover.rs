use crate::{
    bus::{bus_client, BusMessage, SharedMessageBus},
    handle_messages,
    mempool::MempoolEvent,
    model::{BlobTransaction, FeeProofTransaction, Hashable, SharedRunContext, Transaction},
    utils::{logger::LogMe, modules::Module},
};
use anyhow::{bail, Context, Error, Result};
use borsh::to_vec;
use hyle_contract_sdk::{BlobData, Digestable};
use serde::{Deserialize, Serialize};
use tracing::info;

bus_client! {
struct ProverBusClient {
    sender(ProverEvent),
    receiver(MempoolEvent),
}
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ProverEvent {
    NewTx(Transaction),
}
impl BusMessage for ProverEvent {}

pub struct Prover {
    bus: ProverBusClient,
}

impl Module for Prover {
    type Context = SharedRunContext;
    fn name() -> &'static str {
        "Prover"
    }

    async fn build(ctx: Self::Context) -> Result<Self> {
        let bus = ProverBusClient::new_from_bus(ctx.common.bus.new_handle()).await;
        Ok(Prover { bus })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.start()
    }
}

pub const HYFI_BIN: &[u8] = include_bytes!("../contracts/hyfi/hyfi.img");
pub const HYDENTITY_BIN: &[u8] = include_bytes!("../contracts/hydentity/hydentity.img");

impl Prover {
    pub async fn start(&mut self) -> Result<(), Error> {
        handle_messages! {
            on_bus self.bus,
            listen<MempoolEvent> event => {
                _ = self.handle_mempool_event(event)
                    .log_error("Prover: Error while handling mempool event");
            }
        }
    }

    fn handle_mempool_event(&mut self, event: MempoolEvent) -> Result<()> {
        match event {
            MempoolEvent::NewTx(tx) => match tx.transaction_data {
                crate::model::TransactionData::Blob(tx) => self.prove_fees(tx),
                _ => Ok(()),
            },
            _ => Ok(()),
        }
    }

    fn prove_fees(&mut self, tx: BlobTransaction) -> Result<()> {
        info!("Got a new transaction to prove fees: {}", tx.hash());
        let fees = &tx.fees;
        if fees.fee.contract_name.0 != "hyfi" {
            bail!("Unsupported fee contract: {}", fees.fee.contract_name);
        }
        if fees.identity.contract_name.0 != "hydentity" {
            bail!(
                "Unsupported identity contract: {}",
                fees.identity.contract_name
            );
        }
        info!("⚒️  Proving hyfi for transaction: {}", tx.hash());

        let initial_state = hyfi::model::Balances::default();
        let contract_inputs = Self::build_contract_inputs(initial_state, fees.fee.data.clone());
        let fees_proof = Self::prove(contract_inputs, HYFI_BIN)?;

        info!("⚒️  Proving hydentity for transaction: {}", tx.hash());

        let initial_state = hydentity::model::Identities::default();
        let contract_inputs =
            Self::build_contract_inputs(initial_state, fees.identity.data.clone());
        let identities_proof = Self::prove(contract_inputs, HYDENTITY_BIN)?;

        let fee_proof_tx = FeeProofTransaction {
            transactions: vec![tx.hash()],
            fees_proof,
            identities_proof,
        };
        let proof_tx = Transaction::wrap(crate::model::TransactionData::FeeProof(fee_proof_tx));

        info!("🚀 Sending proof tx to mempool: {}", proof_tx.hash());

        _ = self
            .bus
            .send(ProverEvent::NewTx(proof_tx))
            .context("Cannot send message over channel");

        Ok(())
    }

    fn build_contract_inputs<State>(
        initial_state: State,
        data: BlobData,
    ) -> hyle_contract_sdk::ContractInput<State>
    where
        State: Digestable,
    {
        let tx_hash = "".to_string();
        let blobs = vec![data];
        let index = 0;
        hyle_contract_sdk::ContractInput::<State> {
            initial_state,
            tx_hash,
            blobs,
            index,
        }
    }

    fn prove<ContractInput>(contract_input: ContractInput, binary: &[u8]) -> Result<Vec<u8>>
    where
        ContractInput: serde::Serialize,
    {
        let env = risc0_zkvm::ExecutorEnv::builder()
            .write(&contract_input)?
            .build()?;

        let prover = risc0_zkvm::default_prover();
        let prove_info = prover.prove(env, binary)?;

        let receipt = prove_info.receipt;
        let encoded_receipt = to_vec(&receipt).expect("Unable to encode receipt");
        Ok(encoded_receipt)
    }
}
