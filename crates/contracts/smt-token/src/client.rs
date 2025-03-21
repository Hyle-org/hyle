use client_sdk::{
    helpers::risc0::Risc0Prover,
    transaction_builder::{StateUpdater, TxExecutorBuilder},
};
use sdk::ContractName;

use crate::SmtToken;

pub mod metadata {
    pub const SMT_TOKEN_ELF: &[u8] = include_bytes!("../smt-token.img");
    pub const PROGRAM_ID: [u8; 32] = sdk::str_to_u8(include_str!("../smt-token.txt"));
}
use metadata::*;

impl SmtToken {
    pub fn setup_builder<S: StateUpdater>(
        &self,
        contract_name: ContractName,
        builder: &mut TxExecutorBuilder<S>,
    ) {
        builder.init_with(contract_name, Risc0Prover::new(SMT_TOKEN_ELF));
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
//         SmtTokenAction::Transfer {
//             proof: todo!(),
//             sender_account: todo!(),
//             recipient_account: todo!(),
//             amount,
//         },
//         None,
//         None,
//         None,
//     )?;
//     Ok(())
// }

// pub fn transfer_from(
//     builder: &mut ProvableBlobTx,
//     contract_name: ContractName,
//     sender: String,
//     recipient: String,
//     amount: u128,
// ) -> anyhow::Result<()> {
//     builder.add_action(
//         contract_name,
//         SmtTokenAction::TransferFrom {
//             proof: todo!(),
//             owner_account: todo!(),
//             spender: todo!(),
//             recipient_account: todo!(),
//             amount,
//         },
//         None,
//         None,
//         None,
//     )?;
//     Ok(())
// }

// pub fn approve(
//     builder: &mut ProvableBlobTx,
//     contract_name: ContractName,
//     spender: String,
//     amount: u128,
// ) -> anyhow::Result<()> {
//     builder.add_action(
//         contract_name,
//         SmtTokenAction::Approve {
//             proof: todo!(),
//             owner_account: todo!(),
//             spender,
//             amount,
//         },
//         None,
//         None,
//         None,
//     )?;
//     Ok(())
// }
