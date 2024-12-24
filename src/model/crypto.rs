use blst::min_pk::Signature as BlstSignature;
use serde::{Deserialize, Serialize};
use std::fmt::{self, Display};

use crate::model::{ValidatorPublicKey, HASH_DISPLAY_SIZE};

#[derive(
    Debug, Serialize, Deserialize, Clone, bincode::Encode, bincode::Decode, PartialEq, Eq, Hash,
)]
pub struct Signed<T: bincode::Encode, V: bincode::Encode> {
    pub msg: T,
    pub signature: V,
}

#[derive(
    Serialize, Deserialize, Clone, bincode::Encode, bincode::Decode, Default, PartialEq, Eq, Hash,
)]
pub struct Signature(pub Vec<u8>);

#[derive(
    Debug, Serialize, Deserialize, Clone, bincode::Encode, bincode::Decode, PartialEq, Eq, Hash,
)]
pub struct ValidatorSignature {
    pub signature: Signature,
    pub validator: ValidatorPublicKey,
}
pub type SignedByValidator<T> = Signed<T, ValidatorSignature>;

#[derive(
    Debug,
    Default,
    Serialize,
    Deserialize,
    Clone,
    bincode::Encode,
    bincode::Decode,
    PartialEq,
    Eq,
    Hash,
)]
pub struct AggregateSignature {
    pub signature: Signature,
    pub validators: Vec<ValidatorPublicKey>,
}

impl From<BlstSignature> for Signature {
    fn from(sig: BlstSignature) -> Self {
        Signature(sig.compress().as_slice().to_vec())
    }
}

impl fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Signature")
            .field(&hex::encode(&self.0))
            .finish()
    }
}

impl fmt::Display for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            &hex::encode(self.0.get(..HASH_DISPLAY_SIZE).unwrap_or(&self.0))
        )
    }
}

impl<T: Display + bincode::Encode> Display for SignedByValidator<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        _ = write!(f, " --> from validator {}", self.signature.validator);
        write!(f, "")
    }
}
