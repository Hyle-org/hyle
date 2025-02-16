use std::any::Any;

use crate::{execute, UuidTldState};
use client_sdk::{
    helpers::{risc0::Risc0Prover, ClientSdkExecutor},
    transaction_builder::{StateUpdater, TxExecutorBuilder},
};
use sdk::{utils::as_hyle_output, ContractName, HyleOutput};

struct UuidTldPseudoExecutor {}
impl ClientSdkExecutor for UuidTldPseudoExecutor {
    fn execute(
        &self,
        program_input: &sdk::ProgramInput,
    ) -> anyhow::Result<(Box<dyn Any>, HyleOutput)> {
        let mut res = execute(program_input.clone());
        let output = as_hyle_output(program_input.clone(), &mut res);
        match res {
            Ok(res) => Ok((Box::new(res.1.clone()), output)),
            Err(e) => Err(anyhow::anyhow!(e)),
        }
    }
}

pub mod metadata {
    pub const UUID_TLD_ELF: &[u8] = include_bytes!("../uuid-tld.img");
    pub const PROGRAM_ID: [u8; 32] = sdk::str_to_u8(include_str!("../uuid-tld.txt"));
}

impl UuidTldState {
    pub fn setup_builder<S: StateUpdater>(
        &self,
        contract_name: ContractName,
        builder: &mut TxExecutorBuilder<S>,
    ) {
        builder.init_with(
            contract_name,
            UuidTldPseudoExecutor {},
            Risc0Prover::new(metadata::UUID_TLD_ELF),
        );
    }
}
