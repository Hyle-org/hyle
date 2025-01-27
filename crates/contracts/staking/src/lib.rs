use anyhow::Result;
use sdk::{
    caller::{CalleeBlobs, CallerCallee, ExecutionContext, MutCalleeBlobs},
    erc20::ERC20Action,
    info,
    utils::as_hyle_output,
    Blob, BlobIndex, ContractInput, HyleOutput, Identity, StakingAction, StructuredBlobData,
};
use state::{OnChainState, Staking};

#[cfg(feature = "client")]
pub mod client;

pub mod state;

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

    pub fn execute_action(
        &mut self,
        action: StakingAction,
        blobs: &[Blob],
        index: BlobIndex,
    ) -> Result<String, String> {
        match action {
            StakingAction::Stake { amount } => {
                let transfer_action =
                    sdk::utils::parse_structured_blob::<ERC20Action>(blobs, &(index + 1))
                        .ok_or("No transfer blob found".to_string())?;
                match transfer_action.data.parameters {
                    ERC20Action::Transfer {
                        recipient,
                        amount: transfer_amount,
                    } => {
                        if recipient != "staking" {
                            return Err(format!(
                                "Transfer recipient should be 'staking' but was {}",
                                &recipient
                            ));
                        }

                        let transfer_contract = transfer_action.contract_name;
                        if transfer_contract.0 != "hyllar" {
                            return Err(format!(
                                "Only hyllar token are accepted to stake. Got {transfer_contract}."
                            ));
                        }

                        if amount != transfer_amount {
                            return Err(format!(
                                "Transfer amount {transfer_amount} mismatch Stake amount {amount}"
                            ));
                        }
                    }
                    els => {
                        return Err(format!(
                            "Wrong ERC20Action, should be a transfer {:?} to 'staking' but was {:?}",
                            amount, els
                        ));
                    }
                }

                self.state.stake(self.caller().clone(), amount)
            }
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
            if structured_blob.caller == Some(input.index) {
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

    let res = contract.execute_action(action, &input.blobs, input.index);

    assert!(contract.callee_blobs().is_empty());

    let ocs = contract.on_chain_state();
    info!("state: {:?}", contract.state());
    as_hyle_output(input, ocs, res)
}
