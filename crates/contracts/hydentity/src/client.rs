use client_sdk::{
    helpers::{risc0::Risc0Prover, ClientSdkExecutor},
    transaction_builder::{ProvableBlobTx, StateUpdater, TxExecutorBuilder},
};
use sdk::{identity_provider::IdentityAction, BlobData, ContractName, Digestable};

use crate::{execute, metadata::HYDENTITY_ELF, Hydentity};

struct HydentityPseudoExecutor {}
impl ClientSdkExecutor for HydentityPseudoExecutor {
    fn execute(&self, contract_input: &sdk::ContractInput) -> anyhow::Result<sdk::HyleOutput> {
        Ok(execute(contract_input.clone()))
    }
}

impl Hydentity {
    pub fn setup_builder<S: StateUpdater>(
        &self,
        contract_name: ContractName,
        builder: &mut TxExecutorBuilder<S>,
    ) {
        builder.init_with(
            contract_name,
            self.as_digest(),
            HydentityPseudoExecutor {},
            Risc0Prover::new(HYDENTITY_ELF),
        );
    }
}

pub fn verify_identity(
    builder: &mut ProvableBlobTx,
    contract_name: ContractName,
    state: &Hydentity,
    password: String,
) -> anyhow::Result<()> {
    let nonce = state
        .get_nonce(builder.identity.0.as_str())
        .map_err(|e| anyhow::anyhow!(e))?;

    let password = BlobData(password.into_bytes().to_vec());

    builder
        .add_action(
            contract_name,
            IdentityAction::VerifyIdentity {
                account: builder.identity.0.clone(),
                nonce,
            },
            None,
            None,
        )?
        .with_private_blob(move |_| Ok(password.clone()));
    Ok(())
}

pub fn register_identity(
    builder: &mut ProvableBlobTx,
    contract_name: ContractName,
    password: String,
) -> anyhow::Result<()> {
    let password = BlobData(password.into_bytes().to_vec());

    builder
        .add_action(
            contract_name,
            IdentityAction::RegisterIdentity {
                account: builder.identity.0.clone(),
            },
            None,
            None,
        )?
        .with_private_blob(move |_| Ok(password.clone()));
    Ok(())
}
