#![allow(dead_code, unused_variables)]

use std::sync::Arc;

use anyhow::{anyhow, bail, Error, Result};
use blst::min_pk::{
    AggregatePublicKey, AggregateSignature as BlstAggregateSignature, PublicKey, SecretKey,
    Signature as BlstSignature,
};
pub use hyle_model::{AggregateSignature, Signed, SignedByValidator, ValidatorSignature};
use rand::Rng;

use crate::model::ValidatorPublicKey;

#[derive(Clone)]
pub struct BlstCrypto {
    sk: SecretKey,
    validator_pubkey: ValidatorPublicKey,
}
pub type SharedBlstCrypto = Arc<BlstCrypto>;

#[derive(Default)]
struct Aggregates {
    sigs: Vec<BlstSignature>,
    pks: Vec<PublicKey>,
    val: Vec<ValidatorPublicKey>,
}

const DST: &[u8] = b"BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_NUL_";
pub const SIG_SIZE: usize = 48;

impl BlstCrypto {
    pub fn new(validator_name: String) -> Result<Self> {
        // TODO load secret key from keyring or other
        // here basically secret_key <=> validator_id which is very badly secure !
        let validator_name_bytes = validator_name.as_bytes();
        let mut ikm = [0u8; 32];
        let len = std::cmp::min(validator_name_bytes.len(), 32);
        #[allow(clippy::indexing_slicing, reason = "len checked")]
        ikm[..len].copy_from_slice(&validator_name_bytes[..len]);

        let sk = SecretKey::key_gen(&ikm, &[])
            .map_err(|e| anyhow!("Could not generate key: {:?}", e))?;
        let validator_pubkey = as_validator_pubkey(sk.sk_to_pk());

        Ok(BlstCrypto {
            sk,
            validator_pubkey,
        })
    }

    pub fn new_random() -> Result<Self> {
        let mut rng = rand::rng();
        let id: String = (0..32)
            .map(|_| rng.random_range(33..127) as u8 as char) // Caractères imprimables ASCII
            .collect();
        Self::new(id.as_str().into())
    }

    pub fn validator_pubkey(&self) -> &ValidatorPublicKey {
        &self.validator_pubkey
    }

    pub fn sign<T>(&self, msg: T) -> Result<Signed<T, ValidatorSignature>, Error>
    where
        T: borsh::BorshSerialize,
    {
        let signature = self.sign_msg(&msg)?.into();
        Ok(Signed {
            msg,
            signature: ValidatorSignature {
                signature,
                validator: self.validator_pubkey.clone(),
            },
        })
    }

    pub fn verify<T>(msg: &SignedByValidator<T>) -> Result<bool, Error>
    where
        T: borsh::BorshSerialize,
    {
        let pk = PublicKey::uncompress(&msg.signature.validator.0)
            .map_err(|e| anyhow!("Could not parse PublicKey: {:?}", e))?;
        let sig = BlstSignature::uncompress(&msg.signature.signature.0)
            .map_err(|e| anyhow!("Could not parse Signature: {:?}", e))?;
        let encoded = borsh::to_vec(&msg.msg)?;
        Ok(BlstCrypto::verify_bytes(encoded.as_slice(), &sig, &pk))
    }

    pub fn verify_aggregate<T>(msg: &Signed<T, AggregateSignature>) -> Result<bool, Error>
    where
        T: borsh::BorshSerialize,
    {
        let pk = Self::aggregate_validators_pk(&msg.signature.validators)?;
        let sig = BlstSignature::uncompress(&msg.signature.signature.0)
            .map_err(|e| anyhow!("Could not parse Signature: {:?}", e))?;
        let encoded = borsh::to_vec(&msg.msg)?;
        Ok(BlstCrypto::verify_bytes(encoded.as_slice(), &sig, &pk))
    }

    pub fn sign_aggregate<T>(
        &self,
        msg: T,
        aggregates: &[&SignedByValidator<T>],
    ) -> Result<Signed<T, AggregateSignature>, Error>
    where
        T: borsh::BorshSerialize + Clone,
    {
        let self_signed = self.sign(msg.clone())?;
        Self::aggregate(msg, &[aggregates, &[&self_signed]].concat())
    }

    pub fn aggregate<T>(
        msg: T,
        aggregates: &[&SignedByValidator<T>],
    ) -> Result<Signed<T, AggregateSignature>, Error>
    where
        T: borsh::BorshSerialize + Clone,
    {
        match aggregates.len() {
            0 => bail!("No signatures to aggregate"),
            1 => Ok(Signed {
                msg,
                #[allow(clippy::indexing_slicing, reason = "len checked")]
                signature: AggregateSignature {
                    signature: aggregates[0].signature.signature.clone(),
                    validators: vec![aggregates[0].signature.validator.clone()],
                },
            }),
            _ => {
                let Aggregates { sigs, pks, val } = Self::extract_aggregates(aggregates)?;

                let pks_refs: Vec<&PublicKey> = pks.iter().collect();
                let sigs_refs: Vec<&BlstSignature> = sigs.iter().collect();

                let aggregated_pk = AggregatePublicKey::aggregate(&pks_refs, true)
                    .map_err(|e| anyhow!("could not aggregate public keys: {:?}", e))?;

                let aggregated_sig = BlstAggregateSignature::aggregate(&sigs_refs, true)
                    .map_err(|e| anyhow!("could not aggregate signatures: {:?}", e))?;

                let valid = Self::verify_aggregate(&Signed {
                    msg: msg.clone(),
                    signature: AggregateSignature {
                        signature: aggregated_sig.to_signature().into(),
                        validators: vec![as_validator_pubkey(aggregated_pk.to_public_key())],
                    },
                })
                .map_err(|e| anyhow!("Failed for verify new aggregated signature! Reason: {e}"))?;

                if !valid {
                    return Err(anyhow!(
                        "Failed to aggregate signatures into valid one. Messages might be different."
                    ));
                }

                Ok(Signed {
                    msg,
                    signature: AggregateSignature {
                        signature: aggregated_sig.to_signature().into(),
                        validators: val,
                    },
                })
            }
        }
    }

    fn sign_msg<T>(&self, msg: &T) -> Result<BlstSignature>
    where
        T: borsh::BorshSerialize,
    {
        let encoded = borsh::to_vec(msg)?;
        Ok(self.sign_bytes(encoded.as_slice()))
    }

    fn sign_bytes(&self, msg: &[u8]) -> BlstSignature {
        self.sk.sign(msg, DST, &[])
    }

    fn verify_bytes(msg: &[u8], sig: &BlstSignature, pk: &PublicKey) -> bool {
        let err = sig.verify(true, msg, DST, &[], pk, true);

        matches!(err, blst::BLST_ERROR::BLST_SUCCESS)
    }

    /// Given a list of signed messages, returns lists of signatures, public keys and
    /// validators.
    fn extract_aggregates<T>(aggregates: &[&SignedByValidator<T>]) -> Result<Aggregates>
    where
        T: borsh::BorshSerialize + Clone,
    {
        let mut accu = Aggregates::default();

        for s in aggregates {
            let sig = BlstSignature::uncompress(&s.signature.signature.0)
                .map_err(|_| anyhow!("Could not parse Signature"))?;
            let pk = PublicKey::uncompress(&s.signature.validator.0)
                .map_err(|_| anyhow!("Could not parse Public Key"))?;
            let val = s.signature.validator.clone();

            accu.sigs.push(sig);
            accu.pks.push(pk);
            accu.val.push(val);
        }

        Ok(accu)
    }

    fn aggregate_validators_pk(validators: &[ValidatorPublicKey]) -> Result<PublicKey> {
        let pks = validators
            .iter()
            .map(|v| {
                PublicKey::uncompress(v.0.as_slice())
                    .map_err(|e| anyhow!("Could not parse PublicKey: {:?}", e))
            })
            .collect::<Result<Vec<PublicKey>>>()?;

        let pks_refs: Vec<&PublicKey> = pks.iter().collect();

        let pk = AggregatePublicKey::aggregate(pks_refs.as_slice(), true)
            .map_err(|e| anyhow!("could not aggregate public keys: {:?}", e))?;

        Ok(pk.to_public_key())
    }
}

fn as_validator_pubkey(pk: PublicKey) -> ValidatorPublicKey {
    ValidatorPublicKey(pk.compress().as_slice().to_vec())
}

#[cfg(test)]
mod tests {

    use crate::p2p::network::HandshakeNetMessage;

    use super::*;
    #[test]
    fn test_sign_bytes() {
        let crypto = BlstCrypto::new_random().unwrap();
        let msg = b"hello";
        let sig = crypto.sign_bytes(msg);
        let valid = BlstCrypto::verify_bytes(msg, &sig, &crypto.sk.sk_to_pk());
        assert!(valid);
    }

    #[test]
    fn test_sign() {
        let crypto = BlstCrypto::new_random().unwrap();
        let pub_key = ValidatorPublicKey(crypto.sk.sk_to_pk().to_bytes().as_slice().to_vec());
        let msg = HandshakeNetMessage::Ping;
        let signed = crypto.sign(&msg).unwrap();
        let valid = BlstCrypto::verify(&signed).unwrap();
        assert!(valid);
    }

    fn new_signed<T: borsh::BorshSerialize + Clone>(
        msg: T,
    ) -> (SignedByValidator<T>, ValidatorPublicKey) {
        let crypto = BlstCrypto::new_random().unwrap();
        let pub_key = ValidatorPublicKey(crypto.sk.sk_to_pk().to_bytes().as_slice().to_vec());
        (crypto.sign(msg).unwrap(), crypto.validator_pubkey.clone())
    }

    #[test]
    fn test_sign_aggregate() {
        let (s1, pk1) = new_signed(HandshakeNetMessage::Ping);
        let (s2, pk2) = new_signed(HandshakeNetMessage::Ping);
        let (s3, pk3) = new_signed(HandshakeNetMessage::Ping);
        let (_, pk4) = new_signed(HandshakeNetMessage::Ping);

        let crypto = BlstCrypto::new_random().unwrap();
        let aggregates = vec![&s1, &s2, &s3];
        let mut signed = crypto
            .sign_aggregate(HandshakeNetMessage::Ping, aggregates.as_slice())
            .unwrap();

        assert_eq!(
            signed.signature.validators,
            vec![
                pk1.clone(),
                pk2.clone(),
                pk3.clone(),
                crypto.validator_pubkey.clone(),
            ]
        );
        assert!(BlstCrypto::verify_aggregate(&signed).unwrap());

        // ordering should not matter
        signed.signature.validators = vec![
            pk2.clone(),
            pk1.clone(),
            pk3.clone(),
            crypto.validator_pubkey.clone(),
        ];
        assert!(BlstCrypto::verify_aggregate(&signed).unwrap());

        // Wrong validators
        signed.signature.validators = vec![
            pk1.clone(),
            pk2.clone(),
            pk4.clone(),
            crypto.validator_pubkey.clone(),
        ];
        assert!(!BlstCrypto::verify_aggregate(&signed).unwrap());

        // Wrong duplicated validators
        signed.signature.validators = vec![
            pk1.clone(),
            pk1.clone(),
            pk2.clone(),
            pk4.clone(),
            crypto.validator_pubkey.clone(),
        ];
        assert!(!BlstCrypto::verify_aggregate(&signed).unwrap());
    }

    #[test]
    fn test_sign_aggregate_wrong_message() {
        let (s1, pk1) = new_signed(HandshakeNetMessage::Ping);
        let (s2, pk2) = new_signed(HandshakeNetMessage::Ping);
        let (s3, pk3) = new_signed(HandshakeNetMessage::Pong); // different message

        let crypto = BlstCrypto::new_random().unwrap();
        let aggregates = vec![&s1, &s2, &s3];
        let signed = crypto.sign_aggregate(HandshakeNetMessage::Ping, aggregates.as_slice());

        assert!(signed.is_err_and(|e| {
            e.to_string()
                .contains("Failed to aggregate signatures into valid one.")
        }));
    }

    #[test]
    fn test_sign_aggregate_overlap() {
        let (s1, pk1) = new_signed(HandshakeNetMessage::Ping);
        let (s2, pk2) = new_signed(HandshakeNetMessage::Ping);
        let (s3, pk3) = new_signed(HandshakeNetMessage::Ping);
        let (s4, pk4) = new_signed(HandshakeNetMessage::Ping);

        let crypto = BlstCrypto::new_random().unwrap();
        let aggregates = vec![&s1, &s2, &s3, &s2, &s3, &s4];
        let signed = crypto
            .sign_aggregate(HandshakeNetMessage::Ping, aggregates.as_slice())
            .unwrap();
        assert!(BlstCrypto::verify_aggregate(&signed).unwrap());

        assert_eq!(
            signed.signature.validators,
            vec![
                pk1.clone(),
                pk2.clone(),
                pk3.clone(),
                pk2.clone(),
                pk3.clone(),
                pk4.clone(),
                crypto.validator_pubkey.clone(),
            ]
        )
    }
}
