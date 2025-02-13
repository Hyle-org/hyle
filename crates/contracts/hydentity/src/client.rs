use client_sdk::{
    helpers::ClientSdkExecutor,
    transaction_builder::{ProvableBlobTx, StateTrait},
};
use sdk::{
    identity_provider::IdentityAction, utils::as_hyle_output, ContractName, Digestable, HyleOutput,
};

use crate::{execute, Hydentity};

pub mod metadata {
    pub const HYDENTITY_ELF: &[u8] = include_bytes!("../hydentity.img");
    pub const PROGRAM_ID: [u8; 32] = sdk::str_to_u8(include_str!("../hydentity.txt"));
}

pub struct HydentityPseudoExecutor {}
impl ClientSdkExecutor<Hydentity> for HydentityPseudoExecutor {
    fn execute(
        &self,
        contract_input: &sdk::ContractInput<Hydentity>,
    ) -> anyhow::Result<(Box<Hydentity>, HyleOutput)> {
        let mut res = execute(contract_input.clone());
        let output = as_hyle_output(contract_input.clone(), &mut res);
        match res {
            Ok(res) => Ok((Box::new(res.1.clone()), output)),
            Err(e) => Err(anyhow::anyhow!(e)),
        }
    }
}

pub fn verify_identity(
    builder: &mut ProvableBlobTx<Hydentity>,
    contract_name: ContractName,
    state: &Hydentity,
    password: String,
) -> anyhow::Result<()> {
    let nonce = state
        .get_nonce(builder.identity.0.as_str())
        .map_err(|e| anyhow::anyhow!(e))?;

    let password = password.into_bytes().to_vec();

    builder.add_action(
        contract_name,
        IdentityAction::VerifyIdentity {
            account: builder.identity.0.clone(),
            nonce,
        },
        Some(password),
        None,
        None,
    )
}

pub fn register_identity(
    builder: &mut ProvableBlobTx<Hydentity>,
    contract_name: ContractName,
    password: String,
) -> anyhow::Result<()> {
    let password = password.into_bytes().to_vec();

    builder.add_action(
        contract_name,
        IdentityAction::RegisterIdentity {
            account: builder.identity.0.clone(),
        },
        Some(password),
        None,
        None,
    )
}
