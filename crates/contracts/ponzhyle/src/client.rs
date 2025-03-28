use client_sdk::{
    helpers::risc0::Risc0Prover,
    transaction_builder::{ProvableBlobTx, StateUpdater, TxExecutorBuilder},
};
use sdk::ContractName;

use crate::{Ponzhyle, PonzhyleAction};

pub mod metadata {
    pub const PONZHYLE_ELF: &[u8] = include_bytes!("../ponzhyle.img");
    pub const PROGRAM_ID: [u8; 32] = sdk::str_to_u8(include_str!("../ponzhyle.txt"));
}
use metadata::*;

impl Ponzhyle {
    pub fn setup_builder<S: StateUpdater>(
        &self,
        contract_name: ContractName,
        builder: &mut TxExecutorBuilder<S>,
    ) {
        builder.init_with(contract_name, Risc0Prover::new(PONZHYLE_ELF));
    }
}

pub fn send_invite(
    builder: &mut ProvableBlobTx,
    contract_name: ContractName,
    address: String,
) -> anyhow::Result<()> {
    builder.add_action(
        contract_name,
        PonzhyleAction::SendInvite {
            address: address.into(),
        },
        None,
        None,
        None,
    )?;
    Ok(())
}

pub fn redeem_invite(
    builder: &mut ProvableBlobTx,
    contract_name: ContractName,
    referrer: String,
) -> anyhow::Result<()> {
    builder.add_action(
        contract_name,
        PonzhyleAction::RedeemInvite {
            referrer: referrer.into(),
        },
        None,
        None,
        None,
    )?;
    Ok(())
}

pub fn register_nationality(
    builder: &mut ProvableBlobTx,
    contract_name: ContractName,
    nationality: String,
) -> anyhow::Result<()> {
    builder.add_action(
        contract_name,
        PonzhyleAction::RegisterNationality { nationality },
        None,
        None,
        None,
    )?;
    Ok(())
}
