use crate::Twitter;
use client_sdk::{
    helpers::risc0::Risc0Prover,
    transaction_builder::{StateUpdater, TxExecutorBuilder},
};
use sdk::ContractName;

pub mod metadata {
    pub const TWITTER_ELF: &[u8] = include_bytes!("../twitter.img");
    pub const PROGRAM_ID: [u8; 32] = sdk::str_to_u8(include_str!("../twitter.txt"));
}
use metadata::*;

impl Twitter {
    pub fn setup_builder<S: StateUpdater>(
        &self,
        contract_name: ContractName,
        builder: &mut TxExecutorBuilder<S>,
    ) {
        builder.init_with(contract_name, Risc0Prover::new(TWITTER_ELF));
    }
}

// pub fn verify_identity(
//     builder: &mut ProvableBlobTx,
//     contract_name: ContractName,
//     state: &Hydentity,
//     password: String,
// ) -> anyhow::Result<()> {
//     let nonce = state
//         .get_nonce(builder.identity.0.as_str())
//         .map_err(|e| anyhow::anyhow!(e))?;

//     let password = password.into_bytes().to_vec();

//     builder.add_action(
//         contract_name,
//         HydentityAction::VerifyIdentity {
//             account: builder.identity.0.clone(),
//             nonce,
//         },
//         Some(password),
//         None,
//         None,
//     )?;
//     Ok(())
// }

// pub fn register_identity(
//     builder: &mut ProvableBlobTx,
//     contract_name: ContractName,
//     password: String,
// ) -> anyhow::Result<()> {
//     let name = builder.identity.0.clone();
//     let account = Hydentity::build_id(&name, &password);

//     builder.add_action(
//         contract_name,
//         HydentityAction::RegisterIdentity { account },
//         Some(password.into_bytes().to_vec()),
//         None,
//         None,
//     )?;
//     Ok(())
// }
