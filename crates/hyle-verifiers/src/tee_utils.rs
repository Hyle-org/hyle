use std::{borrow::BorrowMut, collections::BTreeMap};

use anyhow::{bail, Context, Error};
use aws_nitro_enclaves_cose::{
    crypto::{Openssl, SigningPublicKey},
    CoseSign1,
};
use borsh::BorshDeserialize;
use hyle_model::HyleOutput;
use openssl::{
    asn1::Asn1Time,
    bn::BigNumContext,
    ec::{EcKey, PointConversionForm},
    pkey::{PKey, Public},
    x509::{X509VerifyResult, X509},
};
use serde_cbor::{value, Value};

#[derive(BorshDeserialize)]
pub struct TeeAttrs {
    pub cose_sign1: Vec<u8>,
    pub hyle_outputs_sig: Vec<u8>,
    pub hyle_outputs: Vec<HyleOutput>,
}

pub fn verify_root_of_trust(
    attestation_doc: &mut BTreeMap<Value, Value>,
    cosesign1: &CoseSign1,
    timestamp: usize,
    root_pkey: &[u8],
) -> Result<(), Error> {
    // verify attestation doc signature
    let enclave_certificate = attestation_doc
        .remove(&"certificate".to_owned().into())
        .ok_or(anyhow::anyhow!("certificate key not found".to_owned(),))?;
    let enclave_certificate = (match enclave_certificate {
        Value::Bytes(b) => Ok(b),
        _ => Err(anyhow::anyhow!(
            "enclave certificate decode failure".to_owned(),
        )),
    })?;
    let enclave_certificate = X509::from_der(&enclave_certificate)
        .map_err(|e| anyhow::anyhow!(format!("leaf der: {e}")))?;
    let pub_key = enclave_certificate
        .public_key()
        .map_err(|e| anyhow::anyhow!(format!("leaf pubkey: {e}")))?;
    let verify_result = cosesign1
        .verify_signature::<Openssl>(&pub_key)
        .map_err(|e| anyhow::anyhow!(format!("leaf signature: {e}")))?;

    if !verify_result {
        return Err(anyhow::anyhow!("leaf signature"));
    }

    // verify certificate chain
    let cabundle = attestation_doc
        .remove(&"cabundle".to_owned().into())
        .ok_or(anyhow::anyhow!(
            "cabundle key not found in attestation doc".to_owned(),
        ))?;
    let mut cabundle = (match cabundle {
        Value::Array(b) => Ok(b),
        _ => Err(anyhow::anyhow!("cabundle decode failure".to_owned(),)),
    })?;
    cabundle.reverse();

    let root_public_key = verify_cert_chain(enclave_certificate, &cabundle, timestamp)?;
    if root_public_key.as_ref() != root_pkey {
        return Err(anyhow::anyhow!("root public key mismatch"));
    }

    Ok(())
}

fn verify_cert_chain(cert: X509, cabundle: &[Value], timestamp: usize) -> Result<Box<[u8]>, Error> {
    let certs = get_all_certs(cert, cabundle)?;

    for i in 0..(certs.len() - 1) {
        let pubkey = certs[i + 1]
            .public_key()
            .map_err(|e| anyhow::anyhow!(format!("pubkey {i}: {e}")))?;
        if !certs[i]
            .verify(&pubkey)
            .map_err(|e| anyhow::anyhow!(format!("signature {i}: {e}")))?
        {
            return Err(anyhow::anyhow!("signature {i}"));
        }
        if certs[i + 1].issued(&certs[i]) != X509VerifyResult::OK {
            return Err(anyhow::anyhow!("issuer or subject {i}"));
        }
        let current_time = Asn1Time::from_unix(timestamp as i64 / 1000)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        if certs[i].not_after() < current_time || certs[i].not_before() > current_time {
            return Err(anyhow::anyhow!("timestamp {i}"));
        }
    }

    let root_cert = certs.last().ok_or(anyhow::anyhow!("root"))?;

    let root_public_key_der = root_cert
        .public_key()
        .map_err(|e| anyhow::anyhow!(format!("root pubkey: {e}")))?
        .public_key_to_der()
        .map_err(|e| anyhow::anyhow!(format!("root pubkey der: {e}")))?;

    let root_public_key = EcKey::public_key_from_der(&root_public_key_der)
        .map_err(|e| anyhow::anyhow!(format!("root pubkey der: {e}")))?;

    let root_public_key_sec1 = root_public_key
        .public_key()
        .to_bytes(
            root_public_key.group(),
            PointConversionForm::UNCOMPRESSED,
            BigNumContext::new()
                .map_err(|e| anyhow::anyhow!(format!("bignum context: {e}")))?
                .borrow_mut(),
        )
        .map_err(|e| anyhow::anyhow!(format!("sec1: {e}")))?;

    Ok(root_public_key_sec1[1..].to_vec().into_boxed_slice())
}

fn get_all_certs(cert: X509, cabundle: &[Value]) -> Result<Box<[X509]>, Error> {
    let mut all_certs = vec![cert];
    for cert in cabundle {
        let cert = (match cert {
            Value::Bytes(b) => Ok(b),
            _ => Err(anyhow::anyhow!("cert decode")),
        })?;
        let cert = X509::from_der(cert).map_err(|e| anyhow::anyhow!(format!("der: {e}")))?;
        all_certs.push(cert);
    }
    Ok(all_certs.into_boxed_slice())
}

pub fn verify_enclave_signature(
    enclave_key: &PKey<Public>,
    hyle_outputs_sig: &[u8],
    hyle_outputs: &[HyleOutput],
) -> Result<(), Error> {
    let encoded_hyle_outputs =
        borsh::to_vec(hyle_outputs).context("could not encode HyleOutput")?;
    let b = enclave_key
        .verify(&encoded_hyle_outputs, hyle_outputs_sig)
        .map_err(|e| anyhow::anyhow!(format!("could not verify enclave signature: {e}")))?;
    if !b {
        bail!("enclave signature verification failed");
    }
    Ok(())
}

pub fn parse_attestation_map(cose_sign1: &CoseSign1) -> Result<BTreeMap<Value, Value>, Error> {
    // Extract payload from CoseSign1
    let payload = cose_sign1
        .get_payload::<Openssl>(None) // TODO: Use the correct key
        .map_err(|e| anyhow::anyhow!(format!("payload issue: {e}")))?;
    let cbor = serde_cbor::from_slice::<Value>(&payload)
        .map_err(|e| anyhow::anyhow!(format!("cbor issue: {e}")))?;

    // Make a map with all values out of attestation
    let attestation_map = value::from_value::<BTreeMap<Value, Value>>(cbor)
        .map_err(|e| anyhow::anyhow!(format!("attestation map issue: {e}")))?;

    Ok(attestation_map)
}
pub fn parse_timestamp(attestation_map: &mut BTreeMap<Value, Value>) -> Result<usize, Error> {
    let timestamp = attestation_map
        .remove(&"timestamp".to_owned().into())
        .ok_or(anyhow::anyhow!(
            "timestamp not found in attestation doc".to_owned(),
        ))?;
    let timestamp = (match timestamp {
        Value::Integer(b) => Ok(b),
        _ => Err(anyhow::anyhow!("timestamp decode failure".to_owned(),)),
    })?;
    let timestamp = timestamp
        .try_into()
        .map_err(|e| anyhow::anyhow!(format!("timestamp: {e}")))?;

    Ok(timestamp)
}

pub fn parse_pcrs(attestation_map: &mut BTreeMap<Value, Value>) -> Result<[u8; 144], Error> {
    let pcrs_arr = attestation_map
        .remove(&"pcrs".to_owned().into())
        .ok_or(anyhow::anyhow!("pcrs not found"))?;
    let mut pcrs_arr = value::from_value::<BTreeMap<Value, Value>>(pcrs_arr)
        .map_err(|e| anyhow::anyhow!(format!("pcrs: {e}")))?;

    let mut concatenated_pcrs = [0u8; 144];
    for i in 0..3 {
        let pcr_value = pcrs_arr
            .remove(&(i as u32).into())
            .ok_or_else(|| anyhow::anyhow!("pcr{i} not found"))?;
        let pcr_bytes = match pcr_value {
            Value::Bytes(b) => b,
            _ => return Err(anyhow::anyhow!("pcr{i} decode failure")),
        };
        if pcr_bytes.len() != 48 {
            return Err(anyhow::anyhow!("pcr{i} not 48 bytes"));
        }
        concatenated_pcrs[i * 48..(i + 1) * 48].copy_from_slice(&pcr_bytes);
    }

    Ok(concatenated_pcrs)
}

pub fn parse_enclave_key(
    attestation_map: &mut BTreeMap<Value, Value>,
) -> Result<PKey<Public>, Error> {
    let public_key = attestation_map
        .remove(&"public_key".to_owned().into())
        .ok_or(anyhow::anyhow!(
            "public key not found in attestation doc".to_owned(),
        ))?;
    let public_key = (match public_key {
        Value::Bytes(b) => Ok(b),
        _ => Err(anyhow::anyhow!("public key decode failure".to_owned(),)),
    })?;

    let public_key = PKey::public_key_from_der(&public_key)
        .map_err(|e| anyhow::anyhow!(format!("public key der: {e}")))?;

    Ok(public_key)
}
