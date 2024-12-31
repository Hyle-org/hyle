use client_sdk::transaction_builder::{TransactionBuilder, TxBuilder};
use sdk::{BlobData, ContractName, Digestable, StateDigest};

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
        let identity = self.0.builder.identity.clone();
        let mut new_state = self.0.state.clone();
        new_state
            .stake(self.0.builder.identity.clone(), amount)
            .map_err(|e| anyhow::anyhow!(e))?;

        self.0
            .builder
            .add_action(
                self.0.contract_name.clone(),
                crate::metadata::STAKING_ELF,
                StakingAction::Stake { amount },
                None,
                None,
            )?
            .with_private_blob(|state: StateDigest| -> anyhow::Result<BlobData> {
                Ok(BlobData(state.0))
            })
            .build_offchain_state(move |state: StateDigest| -> anyhow::Result<StateDigest> {
                let mut state: Staking = state.try_into()?;
                state
                    .stake(identity.clone(), amount)
                    .map_err(|e| anyhow::anyhow!(e))?;
                Ok(state.as_digest())
            });

        Ok(new_state)
    }

    pub fn delegate(&mut self, validator: ValidatorPublicKey) -> anyhow::Result<()> {
        let identity = self.0.builder.identity.clone();

        self.0
            .builder
            .add_action(
                self.0.contract_name.clone(),
                crate::metadata::STAKING_ELF,
                StakingAction::Delegate {
                    validator: validator.clone(),
                },
                None,
                None,
            )?
            .with_private_blob(|state: StateDigest| -> anyhow::Result<BlobData> {
                Ok(BlobData(state.0))
            })
            .build_offchain_state(move |state: StateDigest| -> anyhow::Result<StateDigest> {
                let mut state: Staking = state.try_into()?;
                state
                    .delegate_to(identity.clone(), validator.clone())
                    .map_err(|e| anyhow::anyhow!(e))?;
                Ok(state.as_digest())
            });
        Ok(())
    }
}
