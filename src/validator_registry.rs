//! Information about other validators of the consensus.

use std::{
    collections::HashMap,
    fmt::{self, Debug, Display},
};

use anyhow::{Error, Result};
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::{
    p2p::network::SignedWithId,
    utils::{crypto::BlstCrypto, vec_utils::SequenceOption},
};

#[derive(Serialize, Deserialize, Clone, Encode, Decode, Default, Eq, PartialEq)]
pub struct ValidatorPublicKey(pub Vec<u8>);

impl std::fmt::Debug for ValidatorPublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ValidatorPublicKey")
            .field(&hex::encode(&self.0))
            .finish()
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Default, Hash, Eq, PartialEq)]
pub struct ValidatorId(pub String);

impl Display for ValidatorId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode)]
pub struct ConsensusValidator {
    pub id: ValidatorId,
    pub pub_key: ValidatorPublicKey,
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode)]
pub enum ValidatorRegistryNetMessage {
    NewValidator(ConsensusValidator),
}

#[derive(Serialize, Deserialize, Encode, Decode)]
pub struct ValidatorRegistry {
    pub validators: HashMap<ValidatorId, ConsensusValidator>,
}

impl ValidatorRegistry {
    pub fn new() -> ValidatorRegistry {
        ValidatorRegistry {
            validators: Default::default(),
        }
    }

    pub fn handle_net_message(&mut self, msg: ValidatorRegistryNetMessage) {
        match msg {
            ValidatorRegistryNetMessage::NewValidator(r) => self.add_validator(r.id.clone(), r),
        }
    }

    fn add_validator(&mut self, id: ValidatorId, validator: ConsensusValidator) {
        info!("Adding validator '{}'", id);
        debug!("{:?}", validator);
        self.validators.insert(id, validator);
    }

    pub fn check_signed<T>(&self, msg: &SignedWithId<T>) -> Result<bool, Error>
    where
        T: Encode + Debug + Clone,
    {
        let validators = msg
            .validators
            .iter()
            .map(|v| self.validators.get(v))
            .collect::<Vec<Option<_>>>()
            .sequence();
        match validators {
            Some(vec) => {
                let vec = vec
                    .iter()
                    .map(|validator| validator.pub_key.clone())
                    .collect::<Vec<ValidatorPublicKey>>();

                BlstCrypto::verify(&msg.with_pub_keys(vec))
            }
            None => {
                warn!("Some validators not found in: {:?}", msg.validators);
                Ok(false)
            }
        }
    }
}

impl Default for ValidatorRegistry {
    fn default() -> Self {
        Self::new()
    }
}
