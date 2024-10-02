#![allow(dead_code, unused_variables)]

use std::sync::Arc;

use anyhow::{anyhow, Error, Result};
use blst::min_sig::{
    AggregatePublicKey, AggregateSignature, PublicKey, SecretKey, Signature as BlstSignature,
};

use crate::{
    p2p::network::{self, Signed, SignedWithId, SignedWithKey},
    validator_registry::{ConsensusValidator, ValidatorId, ValidatorPublicKey},
};

#[derive(Clone)]
pub struct BlstCrypto {
    sk: SecretKey,
    validator_pubkey: ValidatorPublicKey,
    validator_id: ValidatorId,
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
    pub fn new(validator_id: ValidatorId) -> Self {
        // TODO load secret key from keyring or other
        // here basically secret_key <=> validator_id which is very badly secure !
        let validator_id_bytes = validator_id.0.as_bytes();
        let mut ikm = [0u8; 32];
        let len = std::cmp::min(validator_id_bytes.len(), 32);
        ikm[..len].copy_from_slice(&validator_id_bytes[..len]);

        let sk = SecretKey::key_gen(&ikm, &[]).unwrap();
        let validator_pubkey = sk.sk_to_pk().into();

        BlstCrypto {
            sk,
            validator_id,
            validator_pubkey,
        }
    }

    pub fn validator_id(&self) -> &ValidatorId {
        &self.validator_id
    }

    pub fn validator_pubkey(&self) -> &ValidatorPublicKey {
        &self.validator_pubkey
    }

    pub fn as_validator(&self) -> ConsensusValidator {
        let pub_key = self.sk.sk_to_pk().into();
        ConsensusValidator {
            id: self.validator_id.clone(),
            pub_key,
        }
    }

    pub fn sign<T>(&self, msg: T) -> Result<SignedWithId<T>, Error>
    where
        T: bincode::Encode,
    {
        let signature = self.sign_msg(&msg)?.into();
        Ok(Signed {
            msg,
            signature,
            validators: vec![self.validator_id.clone()],
        })
    }

    pub fn verify<T>(msg: &SignedWithKey<T>) -> Result<bool, Error>
    where
        T: bincode::Encode,
    {
        let pk = Self::aggregate_validators_pk(&msg.validators)?;
        let sig = BlstSignature::uncompress(&msg.signature.0)
            .map_err(|e| anyhow!("Could not parse Signature: {:?}", e))?;
        let encoded = bincode::encode_to_vec(&msg.msg, bincode::config::standard())?;
        Ok(BlstCrypto::verify_bytes(encoded.as_slice(), &sig, &pk))
    }

    pub fn sign_aggregate<T>(
        &self,
        msg: T,
        aggregates: &[&SignedWithKey<T>],
    ) -> Result<SignedWithKey<T>, Error>
    where
        T: bincode::Encode + Clone,
    {
        let Aggregates { sigs, pks, mut val } = Self::extract_aggregates(aggregates)?;

        val.push(self.validator_pubkey.clone());

        let mut sigs_refs: Vec<&BlstSignature> = sigs.iter().collect::<Vec<&BlstSignature>>();
        let mut pks_refs: Vec<&PublicKey> = pks.iter().collect();

        let self_signed = self.sign_msg(&msg)?;
        let pk = self.sk.sk_to_pk();
        sigs_refs.push(&self_signed);
        pks_refs.push(&pk);

        let pk = AggregatePublicKey::aggregate(&pks_refs, true)
            .map_err(|e| anyhow!("could not aggregate public keys: {:?}", e))?;

        let sig = AggregateSignature::aggregate(&sigs_refs, true)
            .map_err(|e| anyhow!("could not aggregate signatures: {:?}", e))?;

        let valid = Self::verify(&SignedWithKey {
            msg: msg.clone(),
            signature: sig.to_signature().into(),
            validators: vec![pk.to_public_key().into()],
        })
        .map_err(|e| anyhow!("Failed for verify new aggregated signature! Reason: {e}"))?;

        if !valid {
            return Err(anyhow!(
                "Failed to aggregate signatures into valid one. Messages might be different."
            ));
        }

        Ok(Signed {
            msg,
            signature: sig.to_signature().into(),
            validators: val,
        })
    }

    fn sign_msg<T>(&self, msg: &T) -> Result<BlstSignature>
    where
        T: bincode::Encode,
    {
        let encoded = bincode::encode_to_vec(msg, bincode::config::standard())?;
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
    fn extract_aggregates<T>(aggregates: &[&SignedWithKey<T>]) -> Result<Aggregates>
    where
        T: bincode::Encode + Clone,
    {
        let mut accu = Aggregates::default();

        for s in aggregates {
            let sig = BlstSignature::uncompress(&s.signature.0)
                .map_err(|_| anyhow!("Could not parse Signature"))?;
            let pk = Self::aggregate_validators_pk(&s.validators)?;
            let mut val = s.validators.clone();

            accu.sigs.push(sig);
            accu.pks.push(pk);
            accu.val.append(&mut val);
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

impl From<BlstSignature> for network::Signature {
    fn from(sig: BlstSignature) -> Self {
        network::Signature(sig.compress().as_slice().to_vec())
    }
}

impl From<PublicKey> for ValidatorPublicKey {
    fn from(pk: PublicKey) -> Self {
        ValidatorPublicKey(pk.compress().as_slice().to_vec())
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
        let valid = BlstCrypto::verify_bytes(msg, &sig, &crypto.sk.sk_to_pk());
        assert!(valid);
    }

    #[test]
    fn test_sign() {
        let crypto = BlstCrypto::default();
        let pub_key = ValidatorPublicKey(crypto.sk.sk_to_pk().to_bytes().as_slice().to_vec());
        let msg = HandshakeNetMessage::Ping;
        let signed = crypto.sign(&msg).unwrap();
        let valid = BlstCrypto::verify(&signed.with_pub_keys(vec![pub_key])).unwrap();
        assert!(valid);
    }

    fn new_signed<T: bincode::Encode + Clone>(msg: T) -> (SignedWithKey<T>, ValidatorPublicKey) {
        let crypto = BlstCrypto::default();
        let pub_key = ValidatorPublicKey(crypto.sk.sk_to_pk().to_bytes().as_slice().to_vec());
        (
            crypto.sign(msg).unwrap().with_pub_keys(vec![pub_key]),
            crypto.validator_pubkey.clone(),
        )
    }

    #[test]
    fn test_sign_aggregate() {
        let (s1, pk1) = new_signed(HandshakeNetMessage::Ping);
        let (s2, pk2) = new_signed(HandshakeNetMessage::Ping);
        let (s3, pk3) = new_signed(HandshakeNetMessage::Ping);
        let (_, pk4) = new_signed(HandshakeNetMessage::Ping);

        let crypto = BlstCrypto::default();
        let aggregates = vec![&s1, &s2, &s3];
        let mut signed = crypto
            .sign_aggregate(HandshakeNetMessage::Ping, aggregates.as_slice())
            .unwrap();

        assert_eq!(
            signed.validators,
            vec![
                pk1.clone(),
                pk2.clone(),
                pk3.clone(),
                crypto.validator_pubkey.clone(),
            ]
        );
        assert!(BlstCrypto::verify(&signed).unwrap());

        // ordering should not matter
        signed.validators = vec![
            pk2.clone(),
            pk1.clone(),
            pk3.clone(),
            crypto.validator_pubkey.clone(),
        ];
        assert!(BlstCrypto::verify(&signed).unwrap());

        // Wrong validators
        signed.validators = vec![
            pk1.clone(),
            pk2.clone(),
            pk4.clone(),
            crypto.validator_pubkey.clone(),
        ];
        assert!(!BlstCrypto::verify(&signed).unwrap());

        // Wrong duplicated validators
        signed.validators = vec![
            pk1.clone(),
            pk1.clone(),
            pk2.clone(),
            pk4.clone(),
            crypto.validator_pubkey.clone(),
        ];
        assert!(!BlstCrypto::verify(&signed).unwrap());
    }

    #[test]
    fn test_sign_aggregate_wrong_message() {
        let (s1, pk1) = new_signed(HandshakeNetMessage::Ping);
        let (s2, pk2) = new_signed(HandshakeNetMessage::Ping);
        let (s3, pk3) = new_signed(HandshakeNetMessage::Pong); // different message

        let crypto = BlstCrypto::default();
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

        let crypto1 = BlstCrypto::default();
        let aggregates1 = vec![&s1, &s2, &s3];
        let signed1 = crypto1
            .sign_aggregate(HandshakeNetMessage::Ping, aggregates1.as_slice())
            .unwrap();
        assert!(BlstCrypto::verify(&signed1).unwrap());

        let crypto2 = BlstCrypto::default();
        let aggregates2 = vec![&s2, &s3, &s4];
        let signed2 = crypto2
            .sign_aggregate(HandshakeNetMessage::Ping, aggregates2.as_slice())
            .unwrap();
        assert!(BlstCrypto::verify(&signed2).unwrap());

        let crypto = BlstCrypto::default();
        let aggregates = vec![&signed1, &signed2];
        let signed = crypto
            .sign_aggregate(HandshakeNetMessage::Ping, aggregates.as_slice())
            .unwrap();

        assert!(BlstCrypto::verify(&signed).unwrap());

        assert_eq!(
            signed.validators,
            vec![
                pk1.clone(),
                pk2.clone(),
                pk3.clone(),
                crypto1.validator_pubkey.clone(),
                pk2.clone(),
                pk3.clone(),
                pk4.clone(),
                crypto2.validator_pubkey.clone(),
                crypto.validator_pubkey.clone(),
            ]
        )
    }
}
