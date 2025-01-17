use anyhow::Result;
use sdk::{
    caller::{CalleeBlobs, CallerCallee, ExecutionContext, MutCalleeBlobs},
    info,
    utils::as_hyle_output,
    ContractInput, HyleOutput, Identity, StakingAction, StructuredBlobData,
};
use state::{OnChainState, Staking};

#[cfg(feature = "client")]
pub mod client;

pub mod state;

#[cfg(feature = "metadata")]
pub mod metadata {
    pub const STAKING_ELF: &[u8] = include_bytes!("../staking.img");
    pub const PROGRAM_ID: [u8; 32] = sdk::str_to_u8(include_str!("../staking.txt"));
}

pub struct StakingContract {
    exec_ctx: ExecutionContext,
    state: state::Staking,
}

impl CallerCallee for StakingContract {
    fn caller(&self) -> &Identity {
        &self.exec_ctx.caller
    }
    fn callee_blobs(&self) -> CalleeBlobs {
        CalleeBlobs(self.exec_ctx.callees_blobs.borrow())
    }
    fn mut_callee_blobs(&self) -> MutCalleeBlobs {
        MutCalleeBlobs(self.exec_ctx.callees_blobs.borrow_mut())
    }
}

impl StakingContract {
    pub fn new(exec_ctx: ExecutionContext, state: state::Staking) -> Self {
        StakingContract { exec_ctx, state }
    }

    pub fn execute_action(&mut self, action: StakingAction) -> Result<String, String> {
        match action {
            StakingAction::Stake { amount } => self.state.stake(self.caller().clone(), amount),
            StakingAction::Delegate { validator } => {
                self.state.delegate_to(self.caller().clone(), validator)
            }
            StakingAction::Distribute { claim: _ } => todo!(),
        }
    }

    pub fn on_chain_state(&self) -> OnChainState {
        self.state.on_chain_state()
    }

    pub fn state(self) -> state::Staking {
        self.state
    }
}

pub fn execute(contract_input: ContractInput) -> HyleOutput {
    let (input, parsed_blob, caller) =
        match sdk::guest::init_with_caller::<StakingAction>(contract_input) {
            Ok(res) => res,
            Err(err) => {
                panic!("Staking contract initialization failed {}", err);
            }
        };

    // TODO: refactor this into ExecutionContext
    let mut callees_blobs = Vec::new();
    for blob in input.blobs.clone().into_iter() {
        if let Ok(structured_blob) = blob.data.clone().try_into() {
            let structured_blob: StructuredBlobData<Vec<u8>> = structured_blob; // for type inference
            if structured_blob.caller == Some(input.index.clone()) {
                callees_blobs.push(blob);
            }
        };
    }

    let (state, _): (Staking, _) =
        bincode::decode_from_slice(input.private_blob.0.as_slice(), bincode::config::standard())
            .expect("Failed to decode payload");

    let input_initial_state = input
        .initial_state
        .clone()
        .try_into()
        .expect("Failed to decode state");

    info!("state: {:?}", state);
    info!("computed:: {:?}", state.on_chain_state());
    info!("given: {:?}", input_initial_state);
    if state.on_chain_state() != input_initial_state {
        panic!("State mismatch");
    }

    let ctx = ExecutionContext {
        callees_blobs: callees_blobs.into(),
        caller,
    };
    let mut contract = StakingContract::new(ctx, state);

    let action = parsed_blob.data.parameters;

    let res = contract.execute_action(action);

    assert!(contract.callee_blobs().is_empty());

    let ocs = contract.on_chain_state();
    info!("state: {:?}", contract.state());
    as_hyle_output(input, ocs, res)
}
