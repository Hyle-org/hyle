use anyhow::anyhow;
use client_sdk::{
    helpers::{risc0::Risc0Prover, ClientSdkExecutor},
    transaction_builder::{ProvableBlobTx, StateUpdater, TxExecutorBuilder},
};
use sdk::{
    api::{APIFees, APIFeesBalance, APIStaking},
    ContractName, Digestable, StakingAction, StateDigest, ValidatorPublicKey,
};

use crate::{
    execute,
    fees::{Fees, ValidatorFeeState},
    state::Staking,
};

pub mod metadata {
    pub const STAKING_ELF: &[u8] = include_bytes!("../staking.img");
    pub const PROGRAM_ID: [u8; 32] = sdk::str_to_u8(include_str!("../staking.txt"));
}
use metadata::*;

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

impl From<Staking> for APIStaking {
    fn from(val: Staking) -> Self {
        APIStaking {
            stakes: val.stakes,
            rewarded: val.rewarded,
            bonded: val.bonded,
            delegations: val.delegations,
            total_bond: val.total_bond,
            fees: val.fees.into(),
        }
    }
}

impl From<Fees> for APIFees {
    fn from(val: Fees) -> Self {
        APIFees {
            balances: val
                .balances
                .into_iter()
                .map(|(val, b)| {
                    (
                        val,
                        APIFeesBalance {
                            balance: b.balance,
                            cumul_size: b.paid_cumul_size,
                        },
                    )
                })
                .collect(),
        }
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
            fees: val.fees.into(),
        }
    }
}

impl From<APIFees> for Fees {
    fn from(val: APIFees) -> Self {
        Fees {
            pending_fees: vec![],
            balances: val
                .balances
                .into_iter()
                .map(|(val, b)| {
                    (
                        val,
                        ValidatorFeeState {
                            balance: b.balance,
                            paid_cumul_size: b.cumul_size,
                        },
                    )
                })
                .collect(),
        }
    }
}

pub fn deposit_for_fees(
    builder: &mut ProvableBlobTx,
    contract_name: ContractName,
    holder: ValidatorPublicKey,
    amount: u128,
) -> anyhow::Result<()> {
    builder
        .add_action(
            contract_name,
            StakingAction::DepositForFees {
                holder: holder.clone(),
                amount,
            },
            None,
            None,
        )?
        .with_private_input(|state: StateDigest| -> anyhow::Result<Vec<u8>> { Ok(state.0) })
        .build_offchain_state(move |state: StateDigest| -> anyhow::Result<StateDigest> {
            let mut state: Staking = state.try_into()?;
            state
                .deposit_for_fees(holder.clone(), amount)
                .map_err(|e| anyhow!(e))?;
            Ok(state.as_digest())
        });
    Ok(())
}

pub fn stake(
    builder: &mut ProvableBlobTx,
    contract_name: ContractName,
    amount: u128,
) -> anyhow::Result<()> {
    let identity = builder.identity.clone();

    builder
        .add_action(contract_name, StakingAction::Stake { amount }, None, None)?
        .with_private_input(|state: StateDigest| -> anyhow::Result<Vec<u8>> { Ok(state.0) })
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
        .with_private_input(|state: StateDigest| -> anyhow::Result<Vec<u8>> { Ok(state.0) })
        .build_offchain_state(move |state: StateDigest| -> anyhow::Result<StateDigest> {
            let mut state: Staking = state.try_into()?;
            state
                .delegate_to(identity.clone(), validator.clone())
                .map_err(|e| anyhow::anyhow!(e))?;
            Ok(state.as_digest())
        });
    Ok(())
}
