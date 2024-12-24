use client_sdk::transaction_builder::{TransactionBuilder, TxBuilder};
use sdk::{BlobData, ContractName, Digestable};

use crate::{model::ValidatorPublicKey, state::Staking, StakingAction};

pub struct Builder<'a, 'b>(TxBuilder<'a, 'b, Staking>);

impl Staking {
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
    pub fn stake(&mut self, amount: u128) -> anyhow::Result<Staking> {
        let mut new_state = self.0.state.clone();
        new_state
            .stake(self.0.builder.identity.clone(), amount)
            .map_err(|e| anyhow::anyhow!(e))?;

        self.0.builder.add_action(
            self.0.contract_name.clone(),
            crate::metadata::STAKING_ELF,
            self.0.state.on_chain_state(),
            StakingAction::Stake { amount },
            BlobData(self.0.state.as_digest().0),
            None,
            None,
            Some(new_state.as_digest()),
        )?;

        Ok(new_state)
    }

    pub fn delegate(&mut self, validator: ValidatorPublicKey) -> anyhow::Result<()> {
        let mut new_state = self.0.state.clone();
        new_state
            .delegate_to(self.0.builder.identity.clone(), validator.clone())
            .map_err(|e| anyhow::anyhow!(e))?;

        self.0.builder.add_action(
            self.0.contract_name.clone(),
            crate::metadata::STAKING_ELF,
            self.0.state.on_chain_state(),
            StakingAction::Delegate { validator },
            BlobData(self.0.state.as_digest().0),
            None,
            None,
            Some(new_state.as_digest()),
        )
    }
}
