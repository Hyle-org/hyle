use client_sdk::transaction_builder::TransactionBuilder;
use sdk::{BlobData, ContractName, Digestable, StakingAction, StateDigest, ValidatorPublicKey};

use crate::state::Staking;

pub struct Builder<'b> {
    pub contract_name: ContractName,
    pub builder: &'b mut TransactionBuilder,
}

impl Staking {
    pub fn builder<'b>(&self, builder: &'b mut TransactionBuilder) -> Builder<'b> {
        builder.init_with("staking".into(), self.on_chain_state().as_digest());
        Builder {
            contract_name: "staking".into(),
            builder,
        }
    }
}

impl Builder<'_> {
    pub fn stake(&mut self, amount: u128) -> anyhow::Result<()> {
        let identity = self.builder.identity.clone();

        self.builder
            .add_action(
                self.contract_name.clone(),
                crate::metadata::STAKING_ELF,
                client_sdk::helpers::Prover::Risc0Prover,
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

        Ok(())
    }

    pub fn delegate(&mut self, validator: ValidatorPublicKey) -> anyhow::Result<()> {
        let identity = self.builder.identity.clone();

        self.builder
            .add_action(
                self.contract_name.clone(),
                crate::metadata::STAKING_ELF,
                client_sdk::helpers::Prover::Risc0Prover,
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
