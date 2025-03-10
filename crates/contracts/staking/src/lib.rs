use hyllar::HyllarAction;
use sdk::{
    utils::parse_contract_input, Blob, BlobIndex, ContractInput, HyleContract, RunResult,
    StakingAction,
};
use state::Staking;

#[cfg(feature = "client")]
pub mod client;

pub mod fees;
pub mod state;

impl HyleContract for Staking {
    fn execute(&mut self, contract_input: &ContractInput) -> RunResult {
        let (action, execution_ctx) = parse_contract_input::<StakingAction>(contract_input)?;

        let output = match action {
            StakingAction::Stake { amount } => {
                check_transfer_blob(&contract_input.blobs, contract_input.index + 1, amount)?;
                self.stake(execution_ctx.caller.clone(), amount)
            }
            StakingAction::Delegate { validator } => {
                self.delegate_to(execution_ctx.caller.clone(), validator)
            }
            StakingAction::Distribute { claim: _ } => todo!(),
            StakingAction::DepositForFees { holder, amount } => {
                check_transfer_blob(&contract_input.blobs, contract_input.index + 1, amount)?;
                self.deposit_for_fees(holder, amount)
            }
        };

        match output {
            Err(e) => Err(e),
            Ok(output) => Ok((output, execution_ctx, vec![])),
        }
    }
}

fn check_transfer_blob(blobs: &[Blob], index: BlobIndex, amount: u128) -> Result<(), String> {
    let transfer_action = sdk::utils::parse_structured_blob::<HyllarAction>(blobs, &index)
        .ok_or("No transfer blob found".to_string())?;
    match transfer_action.data.parameters {
        HyllarAction::Transfer {
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
            "Wrong HyllarAction, should be a transfer {:?} to 'staking' but was {:?}",
            amount, els
        )),
    }
}
