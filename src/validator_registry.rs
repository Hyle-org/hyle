//! Information about other validators of the consensus.

use std::{
    collections::HashMap,
    fmt::{self, Debug, Display},
    sync::{Arc, RwLock},
};

use anyhow::{Error, Result};
use bincode::{Decode, Encode};
use serde::{
    de::{self, Visitor},
    Deserialize, Serialize,
};
use tracing::{debug, info, warn};

use crate::{bus::BusMessage, p2p::network::SignedWithId, utils::crypto::BlstCrypto};

#[derive(Clone, Encode, Decode, Default, Eq, PartialEq, Hash)]
pub struct ValidatorPublicKey(pub Vec<u8>);

impl std::fmt::Debug for ValidatorPublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ValidatorPublicKey")
            .field(&hex::encode(&self.0))
            .finish()
    }
}

impl Display for ValidatorPublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(&self.0))
    }
}

impl Serialize for ValidatorPublicKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(hex::encode(&self.0).as_str())
    }
}

impl<'de> Deserialize<'de> for ValidatorPublicKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ValidatorPublicKeyVisitor;

        impl<'de> Visitor<'de> for ValidatorPublicKeyVisitor {
            type Value = ValidatorPublicKey;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a hex string representing a ValidatorPublicKey")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                let bytes = hex::decode(value).map_err(de::Error::custom)?;
                Ok(ValidatorPublicKey(bytes))
            }
        }

        deserializer.deserialize_str(ValidatorPublicKeyVisitor)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Default, Hash, Eq, PartialEq)]
pub struct ValidatorId(pub String);

impl Display for ValidatorId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for ValidatorId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Eq, PartialEq)]
pub struct ConsensusValidator {
    pub id: ValidatorId,
    pub pub_key: ValidatorPublicKey,
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Eq, PartialEq)]
pub enum ValidatorRegistryNetMessage {
    NewValidator(ConsensusValidator),
}
impl BusMessage for ValidatorRegistryNetMessage {}

#[derive(Serialize, Deserialize, Encode, Decode)]
pub struct ValidatorRegistryInner {
    pub validators: HashMap<ValidatorId, ConsensusValidator>,
}

pub struct ValidatorRegistry {
    pub inner: Arc<RwLock<ValidatorRegistryInner>>,
}

impl ValidatorRegistry {
    pub fn new(self_validator: ConsensusValidator) -> ValidatorRegistry {
        Self {
            inner: Arc::new(RwLock::new(ValidatorRegistryInner {
                validators: HashMap::from([(self_validator.id.clone(), self_validator)]),
            })),
        }
    }

    pub fn share(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }

    pub fn list_validators(&self) -> Vec<ValidatorPublicKey> {
        self.inner
            .read()
            .unwrap()
            .validators
            .values()
            .map(|v| v.pub_key.clone())
            .collect()
    }

    pub fn get_validators_count(&self) -> usize {
        self.inner.read().unwrap().validators.keys().len()
    }

    pub fn get_pub_keys_from_id(&self, validators_id: Vec<ValidatorId>) -> Vec<ValidatorPublicKey> {
        validators_id
            .into_iter()
            .filter_map(|id| {
                self.inner
                    .read()
                    .unwrap()
                    .validators
                    .get(&id)
                    .map(|validator| validator.pub_key.clone())
            })
            .collect()
    }

    pub fn handle_net_message(&mut self, msg: ValidatorRegistryNetMessage) {
        match msg {
            ValidatorRegistryNetMessage::NewValidator(r) => self.add_validator(r.id.clone(), r),
        }
    }

    pub fn add_validator(&mut self, id: ValidatorId, validator: ConsensusValidator) {
        info!("👋 New validator '{}'", id);
        debug!("{:?}", validator);
        self.inner.write().unwrap().validators.insert(id, validator);
    }

    pub fn check_signed<T>(&self, msg: &SignedWithId<T>) -> Result<bool, Error>
    where
        T: Encode + Debug + Clone,
    {
        debug!("Checking signed message: {:?}", msg);
        let s = &self.inner.read().unwrap().validators;
        let validators = msg
            .validators
            .iter()
            .map(|v| s.get(v))
            .collect::<Option<Vec<_>>>();

        match validators {
            Some(vec) => {
                let vec = vec
                    .iter()
                    .map(|validator| validator.pub_key.clone())
                    .collect::<Vec<ValidatorPublicKey>>();

                debug!("Checking signature with pub keys: {:?}", vec);

                BlstCrypto::verify(&msg.with_pub_keys(vec))
            }
            None => {
                warn!("Some validators not found in: {:?}", msg.validators);
                Ok(false)
            }
        }
    }
}
