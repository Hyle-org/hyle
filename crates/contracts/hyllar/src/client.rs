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

struct HyllarPseudoExecutor {}
impl ClientSdkExecutor for HyllarPseudoExecutor {
    fn execute(
        &self,
        program_inputs: &sdk::ProgramInput,
    ) -> anyhow::Result<(Box<dyn Any>, HyleOutput)> {
        let hyllar_state: HyllarToken =
            borsh::from_slice(&program_inputs.serialized_initial_state)?;
        let contract_input = program_inputs.contract_input.clone();
        let mut res = execute(
            &mut String::new(),
            hyllar_state.clone(),
            contract_input.clone(),
        );
        let output = as_hyle_output(hyllar_state, contract_input, &mut res);
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
