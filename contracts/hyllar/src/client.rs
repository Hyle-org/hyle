use client_sdk::transaction_builder::{TransactionBuilder, TxBuilder};
use sdk::{erc20::ERC20Action, ContractName};

use crate::HyllarToken;

pub struct Builder<'a, 'b>(TxBuilder<'a, 'b, HyllarToken>);

impl HyllarToken {
    pub fn builder<'a, 'b: 'a>(
        &'a self,
        contract_name: ContractName,
        builder: &'b mut TransactionBuilder,
    ) -> Builder {
        Builder(TxBuilder {
            state: self,
            contract_name,
            builder,
        })
    }
}
impl<'a, 'b> Builder<'a, 'b> {
    pub fn transfer(&mut self, recipient: String, amount: u128) -> anyhow::Result<()> {
        self.0.builder.add_action(
            self.0.contract_name.clone(),
            crate::metadata::HYLLAR_ELF,
            ERC20Action::Transfer { recipient, amount },
            None,
            None,
        )?;
        Ok(())
    }
}
