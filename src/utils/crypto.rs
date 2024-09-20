#![allow(dead_code, unused_variables)]
use std::fmt::Debug;

use anyhow::{anyhow, Error, Result};
use blst::min_sig::{PublicKey, SecretKey, Signature};
use rand::RngCore;
use tracing::debug;

use crate::{
    p2p::network::{self, Signed},
    validator_registry::{ConsensusValidator, ValidatorId, ValidatorPublicKey},
};

#[derive(Clone)]
pub struct BlstCrypto {
    sk: SecretKey,
    validator_id: ValidatorId,
}

const DST: &[u8] = b"BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_NUL_";
pub const SIG_SIZE: usize = 48;

impl BlstCrypto {
    pub fn new(validator_id: ValidatorId) -> Self {
        let mut rng = rand::thread_rng();
        let mut ikm = [0u8; 32];
        rng.fill_bytes(&mut ikm);

        let sk = SecretKey::key_gen(&ikm, &[]).unwrap();
        BlstCrypto { sk, validator_id }
    }

    pub fn as_validator(&self) -> ConsensusValidator {
        let pub_key = ValidatorPublicKey(self.sk.sk_to_pk().compress().as_slice().to_vec());
        ConsensusValidator {
            id: self.validator_id.clone(),
            pub_key,
        }
    }

    pub fn sign<T>(&self, msg: T) -> Result<Signed<T>, Error>
    where
        T: bincode::Encode,
    {
        let encoded = bincode::encode_to_vec(&msg, bincode::config::standard())?;
        let signature = network::Signature(self.sign_bytes(encoded.as_slice()).as_slice().to_vec());
        Ok(Signed {
            msg,
            signature,
            validator_id: self.validator_id.clone(),
        })
    }

    pub fn verify<T>(msg: &Signed<T>, pub_key: &ValidatorPublicKey) -> Result<bool, Error>
    where
        T: bincode::Encode + Debug,
    {
        debug!("Verifying message {:?} against {:?}", msg, pub_key);
        let encoded = bincode::encode_to_vec(&msg.msg, bincode::config::standard())?;
        let sig = Signature::uncompress(&msg.signature.0)
            .map_err(|_| anyhow!("Could not parse Signature"))?;
        let pk = PublicKey::uncompress(pub_key.0.as_slice())
            .map_err(|_| anyhow!("Could not parse PublicKey"))?;
        Ok(BlstCrypto::verify_bytes(encoded.as_slice(), &sig, &pk))
    }

    fn sign_bytes(&self, msg: &[u8]) -> [u8; SIG_SIZE] {
        let sig = self.sk.sign(msg, DST, &[]);
        sig.compress()
    }

    fn verify_bytes(msg: &[u8], sig: &Signature, pk: &PublicKey) -> bool {
        let err = sig.verify(true, msg, DST, &[], pk, true);

        matches!(err, blst::BLST_ERROR::BLST_SUCCESS)
    }
}

impl Default for BlstCrypto {
    fn default() -> Self {
        Self::new(ValidatorId("default".to_string()))
    }
}

#[cfg(test)]
mod tests {

    use crate::p2p::network::HandshakeNetMessage;

    use super::*;
    #[test]
    fn test_sign_bytes() {
        let crypto = BlstCrypto::default();
        let msg = b"hello";
        let sig = crypto.sign_bytes(msg);
        let sig = Signature::from_bytes(&sig).unwrap();
        let valid = BlstCrypto::verify_bytes(msg, &sig, &crypto.sk.sk_to_pk());
        assert!(valid);
    }

    #[test]
    fn test_sign() {
        let crypto = BlstCrypto::default();
        let pub_key = ValidatorPublicKey(crypto.sk.sk_to_pk().to_bytes().as_slice().to_vec());
        let msg = HandshakeNetMessage::Ping;
        let signed = crypto.sign(&msg).unwrap();
        BlstCrypto::verify(&signed, &pub_key).unwrap();
    }
}
