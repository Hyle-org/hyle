use client_sdk::{
    helpers::ClientSdkExecutor,
    transaction_builder::{ProvableBlobTx, StateUpdater, TxExecutorBuilder},
};
use sdk::{erc20::ERC20Action, BlobIndex, ContractName, Digestable};

use crate::{execute, AmmAction, AmmState};

struct AmmPseudoExecutor {}
impl ClientSdkExecutor for AmmPseudoExecutor {
    fn execute(&self, contract_input: &sdk::ContractInput) -> anyhow::Result<sdk::HyleOutput> {
        Ok(execute(contract_input.clone()))
    }
}

impl AmmState {
    pub fn setup_builder<S: StateUpdater>(
        &self,
        contract_name: ContractName,
        builder: &mut TxExecutorBuilder,
    ) {
        builder.init_with(
            contract_name,
            self.as_digest(),
            AmmPseudoExecutor {},
            client_sdk::helpers::risc0::Risc0Prover::new(crate::metadata::AMM_ELF),
        );
    }
}

pub fn new_pair(
    builder: &mut ProvableBlobTx,
    contract_name: ContractName,
    pair: (ContractName, ContractName),
    amounts: (u128, u128),
) -> anyhow::Result<()> {
    let idx = builder.blobs.len();
    builder.add_action(
        contract_name.clone(),
        AmmAction::NewPair {
            pair: (pair.0.to_string(), pair.1.to_string()),
            amounts,
        },
        None,
        Some(vec![BlobIndex(idx + 1), BlobIndex(idx + 2)]),
    )?;
    builder.add_action(
        pair.0,
        ERC20Action::TransferFrom {
            sender: builder.identity.0.clone(),
            recipient: contract_name.to_string(),
            amount: amounts.0,
        },
        Some(BlobIndex(idx)),
        None,
    )?;
    builder.add_action(
        pair.1,
        ERC20Action::TransferFrom {
            sender: builder.identity.0.clone(),
            recipient: contract_name.to_string(),
            amount: amounts.1,
        },
        Some(BlobIndex(idx)),
        None,
    )?;
    Ok(())
}

pub fn swap(
    builder: &mut ProvableBlobTx,
    contract_name: ContractName,
    pair: (ContractName, ContractName),
    amounts: (u128, u128),
) -> anyhow::Result<()> {
    let idx = builder.blobs.len();
    builder.add_action(
        contract_name.clone(),
        AmmAction::Swap {
            pair: (pair.0.to_string(), pair.1.to_string()),
            amounts,
        },
        None,
        Some(vec![BlobIndex(idx + 1), BlobIndex(idx + 2)]),
    )?;
    builder.add_action(
        pair.0,
        ERC20Action::TransferFrom {
            sender: builder.identity.0.clone(),
            recipient: contract_name.to_string(),
            amount: amounts.0,
        },
        Some(BlobIndex(idx)),
        None,
    )?;
    builder.add_action(
        pair.1,
        ERC20Action::Transfer {
            recipient: builder.identity.0.clone(),
            amount: amounts.1,
        },
        Some(BlobIndex(idx)),
        None,
    )?;

    tracing::info!("totoro000 {:?}", builder.blobs);
    Ok(())
}
