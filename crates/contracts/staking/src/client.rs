use client_sdk::{
    helpers::risc0::Risc0Prover,
    transaction_builder::{ProvableBlobTx, StateUpdater, TxExecutorBuilder},
};
use sdk::{
    api::{APIFees, APIFeesBalance, APIStaking},
    ContractName, StakingAction, ValidatorPublicKey,
};

use crate::{
    fees::{Fees, ValidatorFeeState},
    state::StakingState,
};

pub mod metadata {
    pub const STAKING_ELF: &[u8] = include_bytes!("../staking.img");
    pub const PROGRAM_ID: [u8; 32] = sdk::str_to_u8(include_str!("../staking.txt"));
}
use metadata::*;

impl StakingState {
    pub fn setup_builder<S: StateUpdater>(
        &self,
        contract_name: ContractName,
        builder: &mut TxExecutorBuilder<S>,
    ) {
        builder.init_with(contract_name, Risc0Prover::new(STAKING_ELF));
    }
}

impl From<StakingState> for APIStaking {
    fn from(val: StakingState) -> Self {
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

impl From<APIStaking> for StakingState {
    fn from(val: APIStaking) -> Self {
        StakingState {
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
    builder.add_action(
        contract_name,
        StakingAction::DepositForFees {
            holder: holder.clone(),
            amount,
        },
        None,
        None,
        None,
    )?;
    Ok(())
}

impl StakingState {
    pub fn to_bytes(&self) -> Vec<u8> {
        borsh::to_vec(self).expect("Failed to encode Balances")
    }
}

pub fn stake(
    builder: &mut ProvableBlobTx,
    contract_name: ContractName,
    amount: u128,
) -> anyhow::Result<()> {
    builder.add_action(
        contract_name,
        StakingAction::Stake { amount },
        None,
        None,
        None,
    )?;
    Ok(())
}

pub fn delegate(
    builder: &mut ProvableBlobTx,
    contract_name: ContractName,
    validator: ValidatorPublicKey,
) -> anyhow::Result<()> {
    builder.add_action(
        contract_name,
        StakingAction::Delegate {
            validator: validator.clone(),
        },
        None,
        None,
        None,
    )?;
    Ok(())
}
