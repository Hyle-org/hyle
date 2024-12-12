use core::fmt;

use bincode::{Decode, Encode};
use serde::{
    de::{self, Visitor},
    Deserialize, Serialize,
};

/// Copied from hyle data model

#[derive(
    Default, Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Hash, Copy, Encode, Decode,
)]
pub struct BlockHeight(pub u64);

#[derive(Clone, Encode, Decode, Default, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub struct ValidatorPublicKey(pub Vec<u8>);

pub const HASH_DISPLAY_SIZE: usize = 3;

impl std::fmt::Debug for ValidatorPublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ValidatorPubK")
            .field(&hex::encode(
                self.0.get(..HASH_DISPLAY_SIZE).unwrap_or(&self.0),
            ))
            .finish()
    }
}

impl fmt::Display for ValidatorPublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            &hex::encode(self.0.get(..HASH_DISPLAY_SIZE).unwrap_or(&self.0),)
        )
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

        impl Visitor<'_> for ValidatorPublicKeyVisitor {
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
