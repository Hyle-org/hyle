use anyhow::{bail, Result};
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fmt::Display};
use tracing::info;

use crate::validator_registry::ValidatorPublicKey;

#[derive(Debug, Encode, Decode, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Staker {
    pub pubkey: ValidatorPublicKey,
    pub stake: Stake,
}

impl Display for Staker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?} with stake {:?}", self.pubkey, self.stake.amount)
    }
}

#[derive(Debug, Encode, Decode, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Stake {
    pub amount: u64,
}

#[derive(Encode, Decode, Default)]
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
    pub fn add_staker(&mut self, staker: Staker) -> Result<()> {
        if self.stakers.contains_key(&staker.pubkey) {
            bail!("Validator already staking")
        }
        info!("💰 New staker {}", staker);
        self.stakers.insert(staker.pubkey, staker.stake);
        Ok(())
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
        info!("🔐 Bonded validator {}", validator);
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

#[cfg(test)]
mod test {

    use super::*;
    #[test]
    fn test_staking() {
        let mut staking = Staking::new();
        let staker = Staker {
            pubkey: ValidatorPublicKey::default(),
            stake: Stake { amount: 100 },
        };
        staking.add_staker(staker.clone());
        assert_eq!(staking.total_bond(), 0);
        let stake = staking.bond(staker.pubkey.clone()).unwrap();
        assert_eq!(staking.total_bond(), 100);
        assert_eq!(stake.amount, 100);
        let stake = staking.unbond(&staker.pubkey).unwrap();
        assert_eq!(staking.total_bond(), 0);
        assert_eq!(stake.amount, 100);
    }

    #[test]
    fn test_errors() {
        let mut staking = Staking::new();
        let staker = Staker {
            pubkey: ValidatorPublicKey::default(),
            stake: Stake { amount: 100 },
        };
        assert!(staking.bond(staker.pubkey.clone()).is_err());
        assert!(staking.unbond(&staker.pubkey).is_err());
        staking.add_staker(staker.clone());
        assert!(staking.bond(staker.pubkey.clone()).is_ok());
        assert!(staking.bond(staker.pubkey.clone()).is_err());
        assert!(staking.unbond(&staker.pubkey).is_ok());
        assert!(staking.unbond(&staker.pubkey).is_err());
    }
}
