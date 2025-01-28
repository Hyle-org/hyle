use std::collections::BTreeMap;

use bincode::{Decode, Encode};
use sdk::{LaneBytesSize, ValidatorPublicKey};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq)]
pub struct ValidatorFeeState {
    /// balance could go negative, the validator would then not be able to
    /// disseminate anymore, and would need to increase its balance first.
    pub(crate) balance: i128,
    /// Cumulative size of the data disseminated by the validator
    pub(crate) cumul_size: LaneBytesSize,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq)]
pub struct Fees {
    /// Cumulative size of the data disseminated by the validators, pending fee distribution
    pub(crate) pending_fees: Vec<(ValidatorPublicKey, LaneBytesSize)>,

    /// Balance of each validator
    pub(crate) balances: BTreeMap<ValidatorPublicKey, ValidatorFeeState>,
}

impl Fees {
    /// Deposit funds to be distributed as fees
    pub fn deposit_for_fees(&mut self, holder: ValidatorPublicKey, amount: u128) {
        self.balances.entry(holder).or_default().balance += amount as i128;
    }

    /// Store the fees to be distributed
    /// DaDi = Data dissemination
    pub(crate) fn pay_for_dadi(
        &mut self,
        disseminator: ValidatorPublicKey,
        cumul_size: LaneBytesSize,
    ) -> Result<(), String> {
        self.pending_fees.push((disseminator, cumul_size));

        Ok(())
    }

    /// Distribute the fees to the bonded validators
    /// The current strategy is quite dummy, it can be improved!
    ///
    /// We could imagine other strategies of distribution, like distributing
    /// the fees for the validators that voted on these DP. For this we would
    /// need to pass the PoDa to the pay_for_dadi function
    pub(crate) fn distribute(&mut self, bonded: &[ValidatorPublicKey]) -> Result<(), String> {
        let fee_per_byte = 1; // TODO: this value could be computed & change over time
        for (disseminator, cumul_size) in self.pending_fees.iter() {
            let fee = cumul_size.0 as i128 * fee_per_byte;

            let Some(disseminator) = self.balances.get_mut(disseminator) else {
                // We should never come here, as the disseminator should have a balance
                // It should be checked by the validator when voting on the DataProposal
                // TODO: I think we sould not fail here, as it will hang the consensus...
                return Err("Logic issue: disseminator not found in balances".to_string());
            };

            disseminator.balance -= fee;
            disseminator.cumul_size = *cumul_size;

            let fee_per_validator = fee / bonded.len() as i128;
            for validator in bonded.iter() {
                let state = self.balances.entry(validator.clone()).or_default();
                state.balance += fee_per_validator;
            }
        }

        Ok(())
    }
}
