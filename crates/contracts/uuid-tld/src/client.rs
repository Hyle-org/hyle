use crate::{execute, UuidTldState};
use client_sdk::{
    helpers::{risc0::Risc0Prover, ClientSdkExecutor},
    transaction_builder::{StateUpdater, TxExecutorBuilder},
};
use sdk::{ContractName, Digestable, StateDigest};

struct UuidTldPseudoExecutor {}
impl ClientSdkExecutor for UuidTldPseudoExecutor {
    fn execute(&self, contract_input: &sdk::ContractInput) -> anyhow::Result<sdk::HyleOutput> {
        Ok(execute(contract_input.clone()))
    }
}

static TEMP: Vec<u8> = vec![];

impl UuidTldState {
    pub fn setup_builder<S: StateUpdater>(
        &self,
        contract_name: ContractName,
        builder: &mut TxExecutorBuilder<S>,
    ) {
        builder.init_with(
            contract_name,
            self.as_digest(),
            UuidTldPseudoExecutor {},
            Risc0Prover::new(&TEMP),
        );
    }
}

impl TryFrom<StateDigest> for UuidTldState {
    type Error = anyhow::Error;

    fn try_from(value: StateDigest) -> anyhow::Result<Self> {
        UuidTldState::deserialize(&value.0).map_err(|e| anyhow::anyhow!(e))
    }
}
