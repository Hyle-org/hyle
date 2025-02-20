use client_sdk::{
    helpers::risc0::Risc0Prover,
    transaction_builder::{ProvableBlobTx, StateUpdater, TxExecutorBuilder},
};
use sdk::{
    api::{APIFees, APIFeesBalance, APIStaking},
    erc20::ERC20Action,
    BlobIndex, ContractName, StakingAction, ValidatorPublicKey,
};

use crate::{
    fees::{Fees, ValidatorFeeState},
    state::Staking,
};

pub mod metadata {
    pub const STAKING_ELF: &[u8] = include_bytes!("../staking.img");
    pub const PROGRAM_ID: [u8; 32] = sdk::str_to_u8(include_str!("../staking.txt"));
}
use metadata::*;

impl Staking {
    pub fn setup_builder<S: StateUpdater>(
        &self,
        contract_name: ContractName,
        builder: &mut TxExecutorBuilder<S>,
    ) {
        builder.init_with(contract_name, Risc0Prover::new(STAKING_ELF));
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
    holder: ValidatorPublicKey,
    amount: u128,
) -> anyhow::Result<()> {
    // FIXME: hardcoded contract names
    let staking_contract_name = ContractName("staking".to_string());
    let token_contract_name = ContractName("hyllar".to_string());
    let idx = builder.blobs.len();
    builder.add_action(
        staking_contract_name.clone(),
        StakingAction::DepositForFees {
            holder: holder.clone(),
            amount,
        },
        None,
        None,
        Some(vec![BlobIndex(idx + 1)]),
    )?;
    builder.add_action(
        token_contract_name,
        ERC20Action::TransferFrom {
            owner: builder.identity.0.clone(),
            recipient: staking_contract_name.0,
            amount,
        },
        None,
        Some(BlobIndex(idx)),
        None,
    )?;
    Ok(())
}

impl Staking {
    pub fn to_bytes(&self) -> Vec<u8> {
        borsh::to_vec(self).expect("Failed to encode Balances")
    }
}

pub fn stake(builder: &mut ProvableBlobTx, amount: u128) -> anyhow::Result<()> {
    // FIXME: hardcoded contract names
    let staking_contract_name = ContractName("staking".to_string());
    let token_contract_name = ContractName("hyllar".to_string());
    let idx = builder.blobs.len();
    builder.add_action(
        staking_contract_name.clone(),
        StakingAction::Stake { amount },
        None,
        None,
        Some(vec![BlobIndex(idx + 1)]),
    )?;
    builder.add_action(
        token_contract_name,
        ERC20Action::TransferFrom {
            owner: builder.identity.0.clone(),
            recipient: staking_contract_name.0,
            amount,
        },
        None,
        Some(BlobIndex(idx)),
        None,
    )?;
    Ok(())
}

pub fn delegate(builder: &mut ProvableBlobTx, validator: ValidatorPublicKey) -> anyhow::Result<()> {
    // FIXME: hardcoded contract names
    let staking_contract_name = ContractName("staking".to_string());
    builder.add_action(
        staking_contract_name,
        StakingAction::Delegate {
            validator: validator.clone(),
        },
        None,
        None,
        None,
    )?;
    Ok(())
}
