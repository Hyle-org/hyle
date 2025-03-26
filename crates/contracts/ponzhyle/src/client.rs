use client_sdk::{
    helpers::risc0::Risc0Prover,
    transaction_builder::{StateUpdater, TxExecutorBuilder},
};
use sdk::ContractName;

use crate::Ponzhyle;

pub mod metadata {
    pub const PONZHYLE_ELF: &[u8] = include_bytes!("../ponzhyle.img");
    pub const PROGRAM_ID: [u8; 32] = sdk::str_to_u8(include_str!("../ponzhyle.txt"));
}
use metadata::*;

impl Ponzhyle {
    pub fn setup_builder<S: StateUpdater>(
        &self,
        contract_name: ContractName,
        builder: &mut TxExecutorBuilder<S>,
    ) {
        builder.init_with(contract_name, Risc0Prover::new(PONZHYLE_ELF));
    }
}

// pub fn transfer(
//     builder: &mut ProvableBlobTx,
//     contract_name: ContractName,
//     recipient: String,
//     amount: u128,
// ) -> anyhow::Result<()> {
//     builder.add_action(
//         contract_name,
//         PonzhyleAction::Transfer { recipient, amount },
//         None,
//         None,
//         None,
//     )?;
//     Ok(())
// }
