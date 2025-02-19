use std::any::Any;

use client_sdk::{
    helpers::{risc0::Risc0Prover, ClientSdkExecutor},
    transaction_builder::{ProvableBlobTx, StateUpdater, TxExecutorBuilder},
};
use sdk::{erc20::ERC20Action, utils::as_hyle_output, BlobIndex, ContractName, Digestable};

use crate::{execute, AmmAction, AmmState};

pub mod metadata {
    pub const AMM_ELF: &[u8] = include_bytes!("../amm.img");
    pub const PROGRAM_ID: [u8; 32] = sdk::str_to_u8(include_str!("../amm.txt"));
}
use metadata::*;

pub struct AmmPseudoExecutor {}
impl ClientSdkExecutor for AmmPseudoExecutor {
    fn execute(
        &self,
        contract_input: &sdk::ContractInput,
    ) -> anyhow::Result<(Box<dyn Any>, sdk::HyleOutput)> {
        let mut res = execute(contract_input.clone());
        let output = as_hyle_output(contract_input.clone(), &mut res);
        match res {
            Ok(res) => Ok((Box::new(res.1.clone()), output)),
            Err(e) => Err(anyhow::anyhow!(e)),
        }
    }
}

impl AmmState {
    pub fn setup_builder<S: StateUpdater>(
        &self,
        contract_name: ContractName,
        builder: &mut TxExecutorBuilder<S>,
    ) {
        builder.init_with(
            contract_name,
            self.as_digest(),
            AmmPseudoExecutor {},
            Risc0Prover::new(AMM_ELF),
        );
    }
}

pub fn new_pair(
    builder: &mut ProvableBlobTx,
    contract_name: ContractName,
    pair: (ContractName, ContractName),
    amounts: (u128, u128),
) -> anyhow::Result<()> {
    let idx = builder.blobs.len();
    builder.add_action(
        contract_name.clone(),
        AmmAction::NewPair {
            pair: (pair.0.to_string(), pair.1.to_string()),
            amounts,
        },
        None,
        Some(vec![BlobIndex(idx + 1), BlobIndex(idx + 2)]),
    )?;
    builder.add_action(
        pair.0,
        ERC20Action::TransferFrom {
            sender: builder.identity.0.clone(),
            recipient: contract_name.to_string(),
            amount: amounts.0,
        },
        Some(BlobIndex(idx)),
        None,
    )?;
    builder.add_action(
        pair.1,
        ERC20Action::TransferFrom {
            sender: builder.identity.0.clone(),
            recipient: contract_name.to_string(),
            amount: amounts.1,
        },
        Some(BlobIndex(idx)),
        None,
    )?;
    Ok(())
}

pub fn swap(
    builder: &mut ProvableBlobTx,
    contract_name: ContractName,
    pair: (ContractName, ContractName),
    amounts: (u128, u128),
) -> anyhow::Result<()> {
    let idx = builder.blobs.len();
    builder.add_action(
        contract_name.clone(),
        AmmAction::Swap {
            pair: (pair.0.to_string(), pair.1.to_string()),
            amounts,
        },
        None,
        Some(vec![BlobIndex(idx + 1), BlobIndex(idx + 2)]),
    )?;

    builder.add_action(
        pair.0,
        ERC20Action::TransferFrom {
            sender: builder.identity.0.clone(),
            recipient: contract_name.to_string(),
            amount: amounts.0,
        },
        Some(BlobIndex(idx)),
        None,
    )?;
    builder.add_action(
        pair.1,
        ERC20Action::Transfer {
            recipient: builder.identity.0.clone(),
            amount: amounts.1,
        },
        Some(BlobIndex(idx)),
        None,
    )?;

    Ok(())
}
