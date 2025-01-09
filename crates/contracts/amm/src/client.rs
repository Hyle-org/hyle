use client_sdk::transaction_builder::TransactionBuilder;
use sdk::{erc20::ERC20Action, BlobData, ContractName};

use crate::AmmState;

impl AmmState {
    pub fn build_swap(
        &self,
        contract_name: ContractName,
        builder: &mut TransactionBuilder,
        recipient: String,
        amount: u128,
    ) -> anyhow::Result<()> {
        builder.add_action(
            contract_name,
            crate::metadata::HYLLAR_ELF,
            self.clone(),
            ERC20Action::Transfer { recipient, amount },
            BlobData::default(),
            None,
            None,
        )
    }
}
