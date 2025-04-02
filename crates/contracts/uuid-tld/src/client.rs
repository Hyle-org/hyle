use crate::UuidTld;
use client_sdk::{
    helpers::risc0::Risc0Prover,
    transaction_builder::{StateUpdater, TxExecutorBuilder},
};
use sdk::{utils::as_hyle_output, Blob, Calldata, ContractName, ProvableContractState, ZkProgram};

pub mod metadata {
    pub const UUID_TLD_ELF: &[u8] = include_bytes!("../uuid-tld.img");
    pub const PROGRAM_ID: [u8; 32] = sdk::str_to_u8(include_str!("../uuid-tld.txt"));
}

impl UuidTld {
    pub fn setup_builder<S: StateUpdater>(
        &self,
        contract_name: ContractName,
        builder: &mut TxExecutorBuilder<S>,
    ) {
        builder.init_with(contract_name, Risc0Prover::new(metadata::UUID_TLD_ELF));
    }
}

impl ProvableContractState for UuidTld {
    fn build_commitment_metadata(&self, _blob: &Blob) -> Result<Vec<u8>, String> {
        borsh::to_vec(self).map_err(|e| e.to_string())
    }
    fn execute_provable(&mut self, calldata: &Calldata) -> Result<sdk::HyleOutput, String> {
        let initial_state_commitment = <Self as ZkProgram>::commit(self);
        let mut res = <Self as ZkProgram>::execute(self, calldata);
        let next_state_commitment = <Self as ZkProgram>::commit(self);
        Ok(as_hyle_output(
            initial_state_commitment,
            next_state_commitment,
            calldata,
            &mut res,
        ))
    }
}
