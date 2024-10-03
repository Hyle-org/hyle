use anyhow::{bail, Result};
use std::collections::HashMap;

use crate::validator_registry::ValidatorPublicKey;

#[derive(Debug, Clone, Copy)]
pub struct Stake {
    amount: u64,
}

pub struct Staking {
    stakers: HashMap<ValidatorPublicKey, Stake>,
    bonded: Vec<ValidatorPublicKey>,
    total_bond: u64,
}

impl Staking {
    pub fn new() -> Self {
        Staking {
            stakers: Default::default(),
            bonded: Default::default(),
            total_bond: 0,
        }
    }

    /// Add a staking validator
    pub fn add_staker(&mut self, validator: ValidatorPublicKey, stake: Stake) {
        self.stakers.insert(validator, stake);
    }

    /// Unbond a staking validator
    pub fn unbond(&mut self, validator: &ValidatorPublicKey) -> Result<Stake> {
        if let Some(stake) = self.stakers.get(validator) {
            if !self.bonded.contains(validator) {
                bail!("Validator already unbonded")
            }
            self.bonded.retain(|v| v != validator);
            self.total_bond -= stake.amount;
            Ok(*stake)
        } else {
            bail!("Validator not staking")
        }
    }

    /// Get the total bonded amount
    pub fn total_bond(&self) -> u64 {
        self.total_bond
    }

    /// Get the stake for a validator
    pub fn get_stake(&self, validator: &ValidatorPublicKey) -> Option<Stake> {
        self.stakers.get(validator).copied()
    }

    /// Bond a staking validator
    pub fn bond(&mut self, validator: ValidatorPublicKey) -> Result<Stake> {
        if let Some(stake) = self.stakers.get(&validator) {
            if self.bonded.contains(&validator) {
                bail!("Validator already bonded")
            }
            self.bonded.push(validator);
            self.total_bond += stake.amount;
            Ok(*stake)
        } else {
            bail!("Validator not staking")
        }
    }
}

impl Default for Staking {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod test {

    use super::*;
    #[test]
    fn test_staking() {
        let mut staking = Staking::new();
        let validator = ValidatorPublicKey::default();
        let stake = Stake { amount: 100 };
        staking.add_staker(validator.clone(), stake);
        assert_eq!(staking.total_bond(), 0);
        let stake = staking.bond(validator.clone()).unwrap();
        assert_eq!(staking.total_bond(), 100);
        assert_eq!(stake.amount, 100);
        let stake = staking.unbond(&validator).unwrap();
        assert_eq!(staking.total_bond(), 0);
        assert_eq!(stake.amount, 100);
    }

    #[test]
    fn test_errors() {
        let mut staking = Staking::new();
        let validator = ValidatorPublicKey::default();
        let stake = Stake { amount: 100 };
        assert!(staking.bond(validator.clone()).is_err());
        assert!(staking.unbond(&validator).is_err());
        staking.add_staker(validator.clone(), stake);
        assert!(staking.bond(validator.clone()).is_ok());
        assert!(staking.bond(validator.clone()).is_err());
        assert!(staking.unbond(&validator).is_ok());
        assert!(staking.unbond(&validator).is_err());
    }
}
