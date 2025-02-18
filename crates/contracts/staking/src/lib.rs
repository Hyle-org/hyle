use sdk::{
    caller::{CalleeBlobs, CallerCallee, CheckCalleeBlobs, ExecutionContext, MutCalleeBlobs},
    erc20::ERC20Action,
    ContractInput, HyleContract, Identity, RunResult, StakingAction,
};
use state::StakingState;

#[cfg(feature = "client")]
pub mod client;

pub mod fees;
pub mod state;

pub struct StakingContract {
    exec_ctx: ExecutionContext,
    state: StakingState,
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

impl HyleContract<StakingState, StakingAction> for StakingContract {
    fn init(state: StakingState, exec_ctx: ExecutionContext) -> Self {
        StakingContract { exec_ctx, state }
    }

    fn execute_action(
        &mut self,
        action: StakingAction,
        _: &ContractInput,
    ) -> RunResult<StakingState> {
        let output = match action {
            StakingAction::Stake { amount } => {
                // Check that a blob for the transfer exists
                self.is_in_callee_blobs(
                    &"staking".into(),
                    ERC20Action::Transfer {
                        recipient: "staking".to_string(),
                        amount,
                    },
                )?;
                self.state.stake(self.caller().clone(), amount)
            }
            StakingAction::Delegate { validator } => {
                self.state.delegate_to(self.caller().clone(), validator)
            }
            StakingAction::Distribute { claim: _ } => todo!(),
            StakingAction::DepositForFees { holder, amount } => {
                // Check that a blob for the transfer exists
                self.is_in_callee_blobs(
                    &"staking".into(),
                    ERC20Action::Transfer {
                        recipient: "staking".to_string(),
                        amount,
                    },
                )?;
                self.state.deposit_for_fees(holder, amount)
            }
        };

        match output {
            Err(e) => Err(e),
            Ok(output) => Ok((output, self.state.clone(), vec![])),
        }
    }

    fn state(self) -> StakingState {
        self.state
    }
}
