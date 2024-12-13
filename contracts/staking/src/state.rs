use std::collections::BTreeMap;

use super::model::{BlockHeight, ValidatorPublicKey};
use anyhow::Result;
use bincode::{Decode, Encode};
use sdk::{info, Digestable, Identity};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq)]
pub struct OnChainState(pub String);

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq)]
pub struct Staking {
    stakes: BTreeMap<Identity, u128>,
    delegations: BTreeMap<ValidatorPublicKey, Vec<Identity>>,
    /// When a validator distribute rewards, it is added in this list to
    /// avoid distributing twice the rewards for a same block
    rewarded: BTreeMap<ValidatorPublicKey, Vec<BlockHeight>>,

    /// List of validators that are part of consensus
    bonded: Vec<ValidatorPublicKey>,
    total_bond: u128,
}

/// Minimal stake necessary to be part of consensus
pub const MIN_STAKE: u128 = 32;

impl Staking {
    pub fn new() -> Self {
        Staking {
            stakes: BTreeMap::new(),
            delegations: BTreeMap::new(),
            rewarded: BTreeMap::new(),
            bonded: Vec::new(),
            total_bond: 0,
        }
    }
    pub fn on_chain_state(&self) -> OnChainState {
        let mut hasher = Sha256::new();
        hasher.update(self.as_digest().0);
        OnChainState(format!("{:x}", hasher.finalize()))
    }

    pub fn bonded(&self) -> &Vec<ValidatorPublicKey> {
        &self.bonded
    }
    /// Get the total bonded amount
    pub fn total_bond(&self) -> u128 {
        self.total_bond
    }
    pub fn is_bonded(&self, pubkey: &ValidatorPublicKey) -> bool {
        self.bonded.iter().any(|v| v == pubkey)
    }

    /// Bond a staking validator
    pub fn bond(&mut self, validator: ValidatorPublicKey) -> Result<(), String> {
        info!("🔐 Bonded validator {}", validator);
        if let Some(stake) = self.get_stake(&validator) {
            if stake < MIN_STAKE {
                return Err("Validator does not have enough stake".to_string());
            }
            self.bonded.push(validator);
            self.bonded.sort(); // TODO insert in order?
            self.total_bond += stake;
            Ok(())
        } else {
            Err("Validator does not have enough stake".to_string())
        }
    }

    pub fn get_stake(&self, validator: &ValidatorPublicKey) -> Option<u128> {
        self.delegations.get(validator).map(|delegations| {
            delegations
                .iter()
                .map(|delegator| self.stakes.get(delegator).unwrap_or(&0))
                .sum()
        })
    }

    pub fn stake(&mut self, staker: Identity, amount: u128) -> Result<String, String> {
        info!("💰 Adding {} to stake for {}", amount, staker);
        self.stakes
            .entry(staker)
            .and_modify(|e| *e += amount)
            .or_insert(amount);
        Ok("Staked".to_string())
    }

    /// Delegate to a validator, or fail if already delegated to another validator
    pub fn delegate_to(
        &mut self,
        staker: Identity,
        validator: ValidatorPublicKey,
    ) -> Result<String, String> {
        if self.delegations.values().flatten().any(|v| v == &staker) {
            return Err("Already delegated".to_string());
        }

        self.delegations
            .entry(validator)
            .and_modify(|e| e.push(staker.clone()))
            .or_insert_with(|| vec![staker]);
        Ok("Delegated".to_string())
    }
}

impl Default for Staking {
    fn default() -> Self {
        Self::new()
    }
}

impl Digestable for OnChainState {
    fn as_digest(&self) -> sdk::StateDigest {
        sdk::StateDigest(
            bincode::encode_to_vec(self, bincode::config::standard())
                .expect("Failed to encode Balances"),
        )
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

//impl TryFrom<sdk::StateDigest> for Staking {
//    type Error = anyhow::Error;
//
//    fn try_from(state: sdk::StateDigest) -> Result<Self, Self::Error> {
//        let (balances, _) = bincode::decode_from_slice(&state.0, bincode::config::standard())
//            .map_err(|_| anyhow::anyhow!("Could not decode start height"))?;
//        Ok(balances)
//    }
//}
