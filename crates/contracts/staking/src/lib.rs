use anyhow::Result;
use sdk::{
    caller::{CalleeBlobs, CallerCallee, ExecutionContext, MutCalleeBlobs},
    erc20::ERC20Action,
    info, Blob, BlobIndex, ContractInput, DropEndOfReader, Identity, RunResult, StakingAction,
    StructuredBlobData,
};
use state::Staking;

#[cfg(feature = "client")]
pub mod client;

pub mod fees;
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
                Self::check_transfer_blob(blobs, index + 1, amount)?;
                self.state.stake(self.caller().clone(), amount)
            }
            StakingAction::Delegate { validator } => {
                self.state.delegate_to(self.caller().clone(), validator)
            }
            StakingAction::Distribute { claim: _ } => todo!(),
            StakingAction::DepositForFees { holder, amount } => {
                Self::check_transfer_blob(blobs, index + 1, amount)?;
                self.state.deposit_for_fees(holder, amount)
            }
        }
    }

    fn check_transfer_blob(blobs: &[Blob], index: BlobIndex, amount: u128) -> Result<(), String> {
        let transfer_action = sdk::utils::parse_structured_blob::<ERC20Action>(blobs, &index)
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

                Ok(())
            }
            els => Err(format!(
                "Wrong ERC20Action, should be a transfer {:?} to 'staking' but was {:?}",
                amount, els
            )),
        }
    }

    pub fn state(self) -> state::Staking {
        self.state
    }
}

pub fn execute(contract_input: ContractInput<Staking>) -> RunResult<Staking> {
    let (input, parsed_blob, caller) =
        match sdk::guest::init_with_caller::<StakingAction, Staking>(contract_input) {
            Ok(res) => res,
            Err(err) => {
                panic!("Staking contract initialization failed {}", err);
            }
        };

    // TODO: refactor this into ExecutionContext
    let mut callees_blobs = Vec::new();
    for blob in input.blobs.clone().into_iter() {
        if let Ok(structured_blob) = blob.data.clone().try_into() {
            let structured_blob: StructuredBlobData<DropEndOfReader> = structured_blob; // for type inference
            if structured_blob.caller == Some(input.index) {
                callees_blobs.push(blob);
            }
        };
    }

    let ctx = ExecutionContext {
        callees_blobs: callees_blobs.into(),
        caller,
    };
    let mut contract = StakingContract::new(ctx, input.initial_state);

    let action = parsed_blob.data.parameters;

    let res = contract.execute_action(action, &input.blobs, input.index)?;

    assert!(contract.callee_blobs().is_empty());

    let state = contract.state();
    info!("state: {:?}", state);

    Ok((res, state, vec![]))
}
