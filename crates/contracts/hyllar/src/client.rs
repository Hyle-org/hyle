use std::any::Any;

use client_sdk::{
    helpers::{risc0::Risc0Prover, ClientSdkExecutor},
    transaction_builder::{ProvableBlobTx, StateUpdater, TxExecutorBuilder},
};
use sdk::{erc20::ERC20Action, utils::as_hyle_output, ContractName, HyleOutput};

use crate::{execute, HyllarToken};

pub mod metadata {
    pub const HYLLAR_ELF: &[u8] = include_bytes!("../hyllar.img");
    pub const PROGRAM_ID: [u8; 32] = sdk::str_to_u8(include_str!("../hyllar.txt"));
}
use metadata::*;

pub struct HyllarPseudoExecutor {}
impl ClientSdkExecutor for HyllarPseudoExecutor {
    fn execute(
        &self,
        contract_input: &sdk::ContractInput,
    ) -> anyhow::Result<(Box<dyn Any>, HyleOutput)> {
        let initial_state: HyllarToken = borsh::from_slice(contract_input.state.as_slice())?;
        let mut res = execute(
            &mut String::new(),
            initial_state.clone(),
            contract_input.clone(),
        );
        let output = as_hyle_output(initial_state, contract_input.clone(), &mut res);
        match res {
            Ok(res) => Ok((Box::new(res.1.clone()), output)),
            Err(e) => Err(anyhow::anyhow!(e)),
        }
    }
}

impl HyllarToken {
    pub fn setup_builder<S: StateUpdater>(
        &self,
        contract_name: ContractName,
        builder: &mut TxExecutorBuilder<S>,
    ) {
        builder.init_with(
            contract_name,
            HyllarPseudoExecutor {},
            Risc0Prover::new(HYLLAR_ELF),
        );
    }
}

pub fn transfer(
    builder: &mut ProvableBlobTx,
    contract_name: ContractName,
    recipient: String,
    amount: u128,
) -> anyhow::Result<()> {
    builder.add_action(
        contract_name,
        ERC20Action::Transfer { recipient, amount },
        None,
        None,
        None,
    )?;
    Ok(())
}

pub fn transfer_from(
    builder: &mut ProvableBlobTx,
    contract_name: ContractName,
    sender: String,
    recipient: String,
    amount: u128,
) -> anyhow::Result<()> {
    builder.add_action(
        contract_name,
        ERC20Action::TransferFrom {
            sender,
            recipient,
            amount,
        },
        None,
        None,
        None,
    )?;
    Ok(())
}

pub fn approve(
    builder: &mut ProvableBlobTx,
    contract_name: ContractName,
    spender: String,
    amount: u128,
) -> anyhow::Result<()> {
    builder.add_action(
        contract_name,
        ERC20Action::Approve { spender, amount },
        None,
        None,
        None,
    )?;
    Ok(())
}
