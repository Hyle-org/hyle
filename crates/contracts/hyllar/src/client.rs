use client_sdk::{
    helpers::{risc0::Risc0Prover, ClientSdkExecutor},
    transaction_builder::{ProvableBlobTx, StateUpdater, TxExecutorBuilder},
};
use sdk::{erc20::ERC20Action, ContractName, Digestable};

use crate::{execute, metadata::HYLLAR_ELF, HyllarToken};

struct HyllarPseudoExecutor {}
impl ClientSdkExecutor for HyllarPseudoExecutor {
    fn execute(&self, contract_input: &sdk::ContractInput) -> anyhow::Result<sdk::HyleOutput> {
        Ok(execute(&mut String::new(), contract_input.clone()))
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
