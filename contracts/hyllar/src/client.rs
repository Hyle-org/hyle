use client_sdk::transaction_builder::TransactionBuilder;
use sdk::{erc20::ERC20Action, ContractName, Digestable};

use crate::HyllarToken;

pub struct Builder<'b> {
    pub contract_name: ContractName,
    pub builder: &'b mut TransactionBuilder,
}

impl HyllarToken {
    pub fn default_builder<'b>(&self, builder: &'b mut TransactionBuilder) -> Builder<'b> {
        builder.init_with("hyllar".into(), self.as_digest());
        Builder {
            contract_name: "hyllar".into(),
            builder,
        }
    }

    pub fn builder<'b>(
        &self,
        contract_name: ContractName,
        builder: &'b mut TransactionBuilder,
    ) -> Builder<'b> {
        builder.init_with(contract_name.clone(), self.as_digest());
        Builder {
            contract_name,
            builder,
        }
    }
}

impl Builder<'_> {
    pub fn transfer(&mut self, recipient: String, amount: u128) -> anyhow::Result<()> {
        self.builder.add_action(
            self.contract_name.clone(),
            crate::metadata::HYLLAR_ELF,
            client_sdk::helpers::Prover::Risc0Prover,
            ERC20Action::Transfer { recipient, amount },
            None,
            None,
        )?;
        Ok(())
    }

    pub fn transfer_test(&mut self, recipient: String, amount: u128) -> anyhow::Result<()> {
        self.builder.add_action(
            self.contract_name.clone(),
            crate::metadata::HYLLAR_ELF,
            client_sdk::helpers::Prover::TestProver,
            ERC20Action::Transfer { recipient, amount },
            None,
            None,
        )?;
        Ok(())
    }
}
