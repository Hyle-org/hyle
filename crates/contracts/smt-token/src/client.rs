use client_sdk::{
    helpers::risc0::Risc0Prover,
    transaction_builder::{ProvableBlobTx, StateUpdater, TxExecutorBuilder},
};
use sdk::ContractName;

use crate::{account::Account, state::SmtTokenState, SmtTokenAction};

pub mod metadata {
    pub const SMT_TOKEN_ELF: &[u8] = include_bytes!("../smt-token.img");
    pub const PROGRAM_ID: [u8; 32] = sdk::str_to_u8(include_str!("../smt-token.txt"));
}
use metadata::*;

impl SmtTokenState {
    pub fn setup_builder<S: StateUpdater>(
        &self,
        contract_name: ContractName,
        builder: &mut TxExecutorBuilder<S>,
    ) {
        builder.init_with(contract_name, Risc0Prover::new(SMT_TOKEN_ELF));
    }

    pub fn transfer(
        &self,
        builder: &mut ProvableBlobTx,
        contract_name: ContractName,
        sender: String,
        recipient: String,
        amount: u128,
    ) -> anyhow::Result<()> {
        let sender_account = match self.get_account(&sender) {
            Ok(Some(account)) => account,
            Ok(None) => return Err(anyhow::anyhow!("Sender account not found")),
            Err(e) => return Err(e),
        };

        let recipient_account = match self.get_account(&recipient) {
            Ok(Some(account)) => account,
            Ok(None) => Account::new(recipient, 0),
            Err(e) => return Err(e),
        };

        let proof = self
            .accounts
            .merkle_proof(vec![sender_account.get_key(), recipient_account.get_key()])?;

        builder.add_action(
            contract_name,
            SmtTokenAction::Transfer {
                proof: proof.into(),
                sender_account,
                recipient_account,
                amount,
            },
            None,
            None,
            None,
        )?;
        Ok(())
    }

    pub fn transfer_from(
        &self,
        builder: &mut ProvableBlobTx,
        contract_name: ContractName,
        owner: String,
        spender: String,
        recipient: String,
        amount: u128,
    ) -> anyhow::Result<()> {
        let owner_account = match self.get_account(&owner) {
            Ok(Some(account)) => account,
            Ok(None) => return Err(anyhow::anyhow!("Sender account not found")),
            Err(e) => return Err(e),
        };

        let recipient_account = match self.get_account(&recipient) {
            Ok(Some(account)) => account,
            Ok(None) => Account::new(recipient, 0),
            Err(e) => return Err(e),
        };

        let proof = self
            .accounts
            .merkle_proof(vec![owner_account.get_key(), recipient_account.get_key()])?;

        builder.add_action(
            contract_name,
            SmtTokenAction::TransferFrom {
                proof: proof.into(),
                owner_account,
                spender,
                recipient_account,
                amount,
            },
            None,
            None,
            None,
        )?;
        Ok(())
    }

    pub fn approve(
        &self,
        builder: &mut ProvableBlobTx,
        contract_name: ContractName,
        owner: String,
        spender: String,
        amount: u128,
    ) -> anyhow::Result<()> {
        let owner_account = match self.get_account(&owner) {
            Ok(Some(account)) => account,
            Ok(None) => return Err(anyhow::anyhow!("Sender account not found")),
            Err(e) => return Err(e),
        };

        let proof = self.accounts.merkle_proof(vec![owner_account.get_key()])?;

        builder.add_action(
            contract_name,
            SmtTokenAction::Approve {
                proof: proof.into(),
                owner_account,
                spender,
                amount,
            },
            None,
            None,
            None,
        )?;
        Ok(())
    }
}
