use client_sdk::{helpers::ClientSdkExecutor, transaction_builder::ProvableBlobTx};
use sdk::{erc20::ERC20Action, utils::as_hyle_output, ContractName, HyleOutput};

use crate::{execute, HyllarToken};

pub mod metadata {
    pub const HYLLAR_ELF: &[u8] = include_bytes!("../hyllar.img");
    pub const PROGRAM_ID: [u8; 32] = sdk::str_to_u8(include_str!("../hyllar.txt"));
}

pub struct HyllarPseudoExecutor {}
impl ClientSdkExecutor<HyllarToken> for HyllarPseudoExecutor {
    fn execute(
        &self,
        contract_input: &sdk::ContractInput<HyllarToken>,
    ) -> anyhow::Result<(Box<HyllarToken>, HyleOutput)> {
        let mut res = execute(&mut String::new(), contract_input.clone());
        let output = as_hyle_output(contract_input.clone(), &mut res);
        match res {
            Ok(res) => Ok((Box::new(res.1.clone()), output)),
            Err(e) => Err(anyhow::anyhow!(e)),
        }
    }
}

pub fn transfer(
    builder: &mut ProvableBlobTx<HyllarToken>,
    contract_name: ContractName,
    recipient: String,
    amount: u128,
) -> anyhow::Result<()> {
    builder.add_action(
        contract_name,
        ERC20Action::Transfer { recipient, amount },
        None,
        None,
        None,
    )
}

pub fn transfer_from(
    builder: &mut ProvableBlobTx<HyllarToken>,
    contract_name: ContractName,
    sender: String,
    recipient: String,
    amount: u128,
) -> anyhow::Result<()> {
    builder.add_action(
        contract_name,
        ERC20Action::TransferFrom {
            sender,
            recipient,
            amount,
        },
        None,
        None,
        None,
    )
}

pub fn approve(
    builder: &mut ProvableBlobTx<HyllarToken>,
    contract_name: ContractName,
    spender: String,
    amount: u128,
) -> anyhow::Result<()> {
    builder.add_action(
        contract_name,
        ERC20Action::Approve { spender, amount },
        None,
        None,
        None,
    )
}
