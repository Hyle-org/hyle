use core::fmt;

use anyhow::Result;
use bincode::{Decode, Encode};
use model::ValidatorPublicKey;
use sdk::{info, Blob, BlobData, ContractName, Digestable};
use serde::{Deserialize, Serialize};

pub mod model;
#[cfg(feature = "metadata")]
pub mod metadata {
    pub const STAKING_ELF: &[u8] = include_bytes!("../staking.img");
    pub const PROGRAM_ID: [u8; 32] = sdk::str_to_u8(include_str!("../staking.txt"));
}

/// Enum representing the actions that can be performed by the IdentityVerification contract.
#[derive(Encode, Decode, Debug, Clone)]
pub enum StakingAction {
    Stake { amount: u128 },
    Delegate { validator: ValidatorPublicKey },
}

impl StakingAction {
    pub fn as_blob(self, contract_name: ContractName) -> Blob {
        Blob {
            contract_name,
            data: BlobData(
                bincode::encode_to_vec(self, bincode::config::standard())
                    .expect("failed to encode program inputs"),
            ),
        }
    }
}

#[derive(Debug, Encode, Decode, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Staker {
    pub pubkey: ValidatorPublicKey,
    pub stake: Stake,
}

impl fmt::Display for Staker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?} with stake {:?}", self.pubkey, self.stake.amount)
    }
}

#[derive(Debug, Encode, Decode, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Stake {
    pub amount: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq, Hash)]
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
    pub fn add_staker(&mut self, staker: Staker) -> Result<(), String> {
        if self.stakers.iter().any(|s| s.pubkey == staker.pubkey) {
            return Err("Validator already staking".to_string());
        }
        info!("💰 New staker {}", staker);
        self.stakers.push(staker);
        Ok(())
    }

    /// list bonded validators
    pub fn bonded(&self) -> &Vec<ValidatorPublicKey> {
        &self.bonded
    }

    /// Unbond a staking validator
    pub fn unbond(&mut self, validator: &ValidatorPublicKey) -> Result<Stake, String> {
        if let Some(staker) = self.stakers.iter().find(|s| &s.pubkey == validator) {
            if !self.bonded.contains(validator) {
                return Err("Validator already unbonded".to_string());
            }
            self.bonded.retain(|v| v != validator);
            self.total_bond -= staker.stake.amount;
            Ok(staker.stake)
        } else {
            Err("Validator not staking".to_string())
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
    pub fn bond(&mut self, validator: ValidatorPublicKey) -> Result<Stake, String> {
        info!("🔐 Bonded validator {}", validator);
        if let Some(staker) = self.stakers.iter().find(|s| s.pubkey == validator) {
            if self.bonded.contains(&validator) {
                return Err("Validator already bonded".to_string());
            }
            self.bonded.push(validator);
            self.bonded.sort(); // TODO insert in order?
            self.total_bond += staker.stake.amount;
            Ok(staker.stake)
        } else {
            Err("Validator not staking".to_string())
        }
    }

    pub fn is_bonded(&self, pubkey: &ValidatorPublicKey) -> bool {
        self.bonded.iter().any(|v| v == pubkey)
    }
}

impl Default for Staking {
    fn default() -> Self {
        Self::new()
    }
}

impl Digestable for Staking {
    fn as_digest(&self) -> sdk::StateDigest {
        sdk::StateDigest(
            bincode::encode_to_vec(self, bincode::config::standard())
                .expect("Failed to encode Balances"),
        )
    }
}

impl TryFrom<sdk::StateDigest> for Staking {
    type Error = anyhow::Error;

    fn try_from(state: sdk::StateDigest) -> Result<Self, Self::Error> {
        let (balances, _) = bincode::decode_from_slice(&state.0, bincode::config::standard())
            .map_err(|_| anyhow::anyhow!("Could not decode start height"))?;
        Ok(balances)
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
