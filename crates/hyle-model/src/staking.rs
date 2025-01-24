use bincode::{Decode, Encode};
use serde::{
    de::{self, Visitor},
    Deserialize, Serialize,
};

use crate::*;

#[derive(Encode, Decode, Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RewardsClaim {
    block_heights: Vec<BlockHeight>,
}

/// Enum representing the actions that can be performed by the IdentityVerification contract.
#[derive(Encode, Decode, Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub enum StakingAction {
    Stake { amount: u128 },
    Delegate { validator: ValidatorPublicKey },
    Distribute { claim: RewardsClaim },
}

/// Represents the operations that can be performed by the consensus
#[derive(Encode, Decode, Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub enum ConsensusStakingAction {
    Bond { candidate: NewValidatorCandidate }, // Bonding a new validator candidate
}

impl From<NewValidatorCandidate> for ConsensusStakingAction {
    fn from(val: NewValidatorCandidate) -> Self {
        ConsensusStakingAction::Bond { candidate: val }
    }
}

impl ContractAction for StakingAction {
    fn as_blob(
        &self,
        contract_name: ContractName,
        caller: Option<BlobIndex>,
        callees: Option<Vec<BlobIndex>>,
    ) -> Blob {
        Blob {
            contract_name,
            data: BlobData::from(StructuredBlobData {
                caller,
                callees,
                parameters: self.clone(),
            }),
        }
    }
}

#[derive(Clone, Encode, Decode, Default, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub struct ValidatorPublicKey(pub Vec<u8>);

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

        impl Visitor<'_> for ValidatorPublicKeyVisitor {
            type Value = ValidatorPublicKey;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
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

impl std::fmt::Debug for ValidatorPublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ValidatorPubK")
            .field(&hex::encode(
                self.0.get(..HASH_DISPLAY_SIZE).unwrap_or(&self.0),
            ))
            .finish()
    }
}

impl std::fmt::Display for ValidatorPublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            &hex::encode(self.0.get(..HASH_DISPLAY_SIZE).unwrap_or(&self.0),)
        )
    }
}
