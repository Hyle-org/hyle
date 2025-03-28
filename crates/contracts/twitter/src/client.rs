use crate::{Twitter, TwitterAction};
use client_sdk::{
    helpers::risc0::Risc0Prover,
    transaction_builder::{ProvableBlobTx, StateUpdater, TxExecutorBuilder},
};
use sdk::ContractName;

pub mod metadata {
    pub const TWITTER_ELF: &[u8] = include_bytes!("../twitter.img");
    pub const PROGRAM_ID: [u8; 32] = sdk::str_to_u8(include_str!("../twitter.txt"));
}
use metadata::*;

impl Twitter {
    pub fn setup_builder<S: StateUpdater>(
        &self,
        contract_name: ContractName,
        builder: &mut TxExecutorBuilder<S>,
    ) {
        builder.init_with(contract_name, Risc0Prover::new(TWITTER_ELF));
    }
}

pub fn verify_identity(
    builder: &mut ProvableBlobTx,
    contract_name: ContractName,
) -> anyhow::Result<()> {
    builder.add_action(
        contract_name,
        TwitterAction::VerifyHandle {
            handle: builder.identity.0.clone(),
        },
        None,
        None,
        None,
    )?;
    Ok(())
}
