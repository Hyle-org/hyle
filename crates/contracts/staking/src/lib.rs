use sdk::{
    erc20::ERC20Action, utils::parse_contract_input, ContractInput, ContractName, HyleContract,
    RunResult, StakingAction,
};
use state::Staking;

#[cfg(feature = "client")]
pub mod client;

pub mod fees;
pub mod state;

impl HyleContract for Staking {
    fn execute_action(&mut self, contract_input: &ContractInput) -> RunResult {
        let (action, execution_ctx) = parse_contract_input::<StakingAction>(contract_input)?;

        // FIXME: hardcoded contract names
        let staking_contract_name = ContractName("staking".to_string());
        let token_contract_name = ContractName("hyllar".to_string());

        let output = match action {
            StakingAction::Stake { amount } => {
                // Check that a blob for the transfer exists
                execution_ctx.is_in_callee_blobs(
                    &token_contract_name,
                    ERC20Action::TransferFrom {
                        owner: contract_input.identity.0.clone(),
                        recipient: staking_contract_name.0,
                        amount,
                    },
                )?;
                self.stake(execution_ctx.caller().clone(), amount)
            }
            StakingAction::Delegate { validator } => {
                self.delegate_to(execution_ctx.caller().clone(), validator)
            }
            StakingAction::Distribute { claim: _ } => todo!(),
            StakingAction::DepositForFees { holder, amount } => {
                // Check that a blob for the transfer exists
                execution_ctx.is_in_callee_blobs(
                    &token_contract_name,
                    ERC20Action::TransferFrom {
                        owner: contract_input.identity.0.clone(),
                        recipient: staking_contract_name.0,
                        amount,
                    },
                )?;
                self.deposit_for_fees(holder, amount)
            }
        };

        match output {
            Err(e) => Err(e),
            Ok(output) => Ok((output, execution_ctx, vec![])),
        }
    }
}
