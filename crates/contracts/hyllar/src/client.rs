use std::any::Any;

use client_sdk::{
    helpers::{risc0::Risc0Prover, ClientSdkExecutor},
    transaction_builder::{ProvableBlobTx, StateUpdater, TxExecutorBuilder},
};
use sdk::{erc20::ERC20Action, utils::as_hyle_output, ContractName, Digestable, HyleOutput};

use crate::{execute, HyllarToken};

pub mod metadata {
    pub const HYLLAR_ELF: &[u8] = include_bytes!("../hyllar.img");
    pub const PROGRAM_ID: [u8; 32] = sdk::str_to_u8(include_str!("../hyllar.txt"));
}
use metadata::*;

impl TryFrom<sdk::StateDigest> for HyllarToken {
    type Error = anyhow::Error;

    fn try_from(state: sdk::StateDigest) -> Result<Self, Self::Error> {
        let (balances, _) = bincode::decode_from_slice(&state.0, bincode::config::standard())
            .map_err(|_| anyhow::anyhow!("Could not decode hyllar state"))?;
        Ok(balances)
    }
}

struct HyllarPseudoExecutor {}
impl ClientSdkExecutor for HyllarPseudoExecutor {
    fn execute(
        &self,
        contract_input: &sdk::ContractInput,
    ) -> anyhow::Result<(Box<dyn Any>, HyleOutput)> {
        let mut res = execute(&mut String::new(), contract_input.clone());
        let output = as_hyle_output(contract_input.clone(), &mut res);
        match res {
            Ok(res) => Ok((Box::new(res.1.state.clone()), output)),
            Err(e) => Err(anyhow::anyhow!(e)),
        }
    }
}

impl HyllarToken {
    pub fn as_bytes(&self) -> Vec<u8> {
        bincode::encode_to_vec(self, bincode::config::standard())
            .expect("Failed to encode Balances")
    }
    pub fn setup_builder<S: StateUpdater>(
        &self,
        contract_name: ContractName,
        builder: &mut TxExecutorBuilder<S>,
    ) {
        builder.init_with(
            contract_name,
            self.as_digest(),
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
    )?;
    Ok(())
}
