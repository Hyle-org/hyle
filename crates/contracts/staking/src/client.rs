use client_sdk::{
    helpers::{risc0::Risc0Prover, ClientSdkExecutor},
    transaction_builder::{ProvableBlobTx, StateUpdater, TxExecutorBuilder},
};
use sdk::{
    api::APIStaking, BlobData, ContractName, Digestable, StakingAction, StateDigest,
    ValidatorPublicKey,
};

use crate::{execute, metadata::STAKING_ELF, state::Staking};

struct StakingPseudoExecutor {}

impl ClientSdkExecutor for StakingPseudoExecutor {
    fn execute(&self, contract_input: &sdk::ContractInput) -> anyhow::Result<sdk::HyleOutput> {
        Ok(execute(contract_input.clone()))
    }
}

impl Staking {
    pub fn setup_builder<S: StateUpdater>(
        &self,
        contract_name: ContractName,
        builder: &mut TxExecutorBuilder<S>,
    ) {
        builder.init_with(
            contract_name,
            self.on_chain_state().as_digest(),
            StakingPseudoExecutor {},
            Risc0Prover::new(STAKING_ELF),
        );
    }
}

impl From<APIStaking> for Staking {
    fn from(val: APIStaking) -> Self {
        Staking {
            stakes: val.stakes,
            rewarded: val.rewarded,
            bonded: val.bonded,
            delegations: val.delegations,
            total_bond: val.total_bond,
        }
    }
}

pub fn stake(
    builder: &mut ProvableBlobTx,
    contract_name: ContractName,
    amount: u128,
) -> anyhow::Result<()> {
    let identity = builder.identity.clone();

    builder
        .add_action(contract_name, StakingAction::Stake { amount }, None, None)?
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

pub fn delegate(
    builder: &mut ProvableBlobTx,
    contract_name: ContractName,
    validator: ValidatorPublicKey,
) -> anyhow::Result<()> {
    let identity = builder.identity.clone();

    builder
        .add_action(
            contract_name,
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
