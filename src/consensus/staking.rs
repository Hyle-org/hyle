use anyhow::{bail, Result};
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use std::fmt::Display;
use tracing::info;

use crate::model::ValidatorPublicKey;

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

#[derive(Debug, Default, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq, Hash)]
pub struct Staking {
    stakers: Vec<Staker>,
    bonded: Vec<ValidatorPublicKey>,
    total_bond: u64,
}

/// Minimal stake necessary to be part of consensus
pub const MIN_STAKE: u64 = 32;

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
        if self.stakers.iter().any(|s| s.pubkey == staker.pubkey) {
            bail!("Validator already staking")
        }
        info!("💰 New staker {}", staker);
        self.stakers.push(staker);
        Ok(())
    }

    /// Unbond a staking validator
    pub fn unbond(&mut self, validator: &ValidatorPublicKey) -> Result<Stake> {
        if let Some(staker) = self.stakers.iter().find(|s| &s.pubkey == validator) {
            if !self.bonded.contains(validator) {
                bail!("Validator already unbonded")
            }
            self.bonded.retain(|v| v != validator);
            self.total_bond -= staker.stake.amount;
            Ok(staker.stake)
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
        self.stakers
            .iter()
            .find(|s| &s.pubkey == validator)
            .map(|s| s.stake)
    }

    /// Bond a staking validator
    pub fn bond(&mut self, validator: ValidatorPublicKey) -> Result<Stake> {
        info!("🔐 Bonded validator {}", validator);
        if let Some(staker) = self.stakers.iter().find(|s| s.pubkey == validator) {
            if self.bonded.contains(&validator) {
                bail!("Validator already bonded")
            }
            self.bonded.push(validator);
            self.total_bond += staker.stake.amount;
            Ok(staker.stake)
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
        assert!(staking.add_staker(staker.clone()).is_ok());
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
        assert!(staking.add_staker(staker.clone()).is_ok());
        assert!(staking.bond(staker.pubkey.clone()).is_ok());
        assert!(staking.bond(staker.pubkey.clone()).is_err());
        assert!(staking.unbond(&staker.pubkey).is_ok());
        assert!(staking.unbond(&staker.pubkey).is_err());
    }
}
