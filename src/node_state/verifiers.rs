use std::{collections::VecDeque, io::Read};

use anyhow::{bail, Error};
use borsh::from_slice;
use cairo_platinum_prover::air::verify_cairo_proof;
use rand::Rng;
use risc0_zkvm::sha::Digest;
use stark_platinum_prover::proof::options::{ProofOptions, SecurityLevel};

use crate::model::ProofTransaction;
use hyle_contract_sdk::{BlobIndex, HyleOutput, StateDigest, TxHash};

pub fn verify_proof(
    tx: &ProofTransaction,
    verifier: &str,
    program_id: &[u8],
) -> Result<HyleOutput, Error> {
    // TODO: remove test
    match verifier {
        "test" => Ok(serde_json::from_slice(&tx.proof.to_bytes()?)?),
        "cairo" => cairo_proof_verifier(&tx.proof.to_bytes()?),
        "risc0" => risc0_proof_verifier(&tx.proof.to_bytes()?, program_id),
        "noir" => noir_proof_verifier(&tx.proof.to_bytes()?, program_id),
        _ => bail!("{} verifier not implemented yet", verifier),
    }
}

pub fn cairo_proof_verifier(proof: &Vec<u8>) -> Result<HyleOutput, Error> {
    let proof_options = ProofOptions::new_secure(SecurityLevel::Conjecturable100Bits, 3);

    let mut bytes = proof.as_slice();
    if bytes.len() < 8 {
        bail!("Cairo proof is too short");
    }

    // Proof len was stored as an u32, 4u8 needs to be read
    let proof_len = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;

    bytes = &bytes[4..];
    if bytes.len() < proof_len {
        bail!("Cairo proof is not correctly formed");
    }

    let proof = match bincode::serde::decode_from_slice(
        &bytes[0..proof_len],
        bincode::config::standard(),
    ) {
        Ok((proof, _)) => proof,
        Err(e) => {
            bail!("Error while decoding cairo proof. Decode error: {}", e);
        }
    };

    // PublicInputs len was stored as an u32, 4u8 needs to be read
    let pub_inputs_len =
        u32::from_le_bytes(bytes[proof_len..proof_len + 4].try_into().unwrap()) as usize;
    let pub_inputs_bytes = &bytes[proof_len + 4..proof_len + 4 + pub_inputs_len];

    let pub_inputs =
        match bincode::serde::decode_from_slice(pub_inputs_bytes, bincode::config::standard()) {
            Ok((pub_inputs, _)) => pub_inputs,
            Err(e) => {
                bail!(
                    "Error while decoding cairo proof's public input. Decode error: {}",
                    e
                );
            }
        };
    let program_output_bytes = &bytes[proof_len + 4 + pub_inputs_len..];

    let program_output = match bincode::serde::decode_from_slice::<HyleOutput, _>(
        program_output_bytes,
        bincode::config::standard(),
    ) {
        Ok((program_output, _)) => program_output,
        Err(e) => {
            bail!(
                "Error while decoding cairo proof's output. Decode error: {}",
                e
            );
        }
    };

    if verify_cairo_proof(&proof, &pub_inputs, &proof_options) {
        Ok(program_output)
    } else {
        bail!("Cairo proof verification failed.");
    }
}

pub fn risc0_proof_verifier(encoded_receipt: &[u8], image_id: &[u8]) -> Result<HyleOutput, Error> {
    let receipt = match from_slice::<risc0_zkvm::Receipt>(encoded_receipt) {
        Ok(v) => v,
        Err(e) => bail!(
            "Error while decoding Risc0 proof's receipt. Decode error: {}",
            e
        ),
    };

    let image_bytes: Digest = image_id.try_into().expect("Invalid Risc0 image ID");

    match receipt.verify(image_bytes) {
        Ok(_) => (),
        Err(e) => bail!("Risc0 proof verification failed: {}", e),
    };

    let hyle_output = match receipt.journal.decode::<HyleOutput>() {
        Ok(v) => v,
        Err(e) => bail!("Failed to extract HyleOuput from Risc0's journal: {}", e),
    };

    tracing::info!(
        "✅ Risc0 proof verified. {}",
        std::str::from_utf8(&hyle_output.program_outputs)
            .map(|o| format!("Program outputs: {o}"))
            .unwrap_or("Invalid UTF-8".to_string())
    );

    // // TODO: allow multiple outputs when verifying
    Ok(hyle_output)
}

/// At present, we are using binary to facilitate the integration of the Noir verifier.
/// This is not meant to be a permanent solution.
pub fn noir_proof_verifier(proof: &[u8], image_id: &[u8]) -> Result<HyleOutput, Error> {
    let mut rng = rand::thread_rng();
    let salt: [u8; 16] = rng.gen();

    let proof_path = &format!("/tmp/noir-proof-{:?}", salt);
    let vk_path = &format!("/tmp/noir-vk-{:?}", salt);
    let output_path = &format!("/tmp/noir-output-{:?}", salt);

    // Write proof and publicKey to files
    std::fs::write(proof_path, proof)?;
    std::fs::write(vk_path, image_id)?;

    // Verifying proof
    let verification_output = std::process::Command::new("bb")
        .arg("verify")
        .arg("-p")
        .arg(proof_path)
        .arg("-k")
        .arg(vk_path)
        .output()?;

    if !verification_output.status.success() {
        bail!(
            "Noir proof verification failed: {}",
            String::from_utf8_lossy(&verification_output.stderr)
        );
    }

    // Extracting outputs
    let public_outputs_output = std::process::Command::new("bb")
        .arg("proof_as_fields")
        .arg("-p")
        .arg(proof_path)
        .arg("-k")
        .arg(vk_path)
        .arg("-o")
        .arg(output_path)
        .output()?;

    if !public_outputs_output.status.success() {
        bail!(
            "Could not extract output from Noir proof: {}",
            String::from_utf8_lossy(&verification_output.stderr)
        );
    }

    // Reading output
    let mut file = std::fs::File::open(output_path).expect("Failed to open output file");
    let mut output_json = String::new();
    file.read_to_string(&mut output_json)
        .expect("Failed to read output file content");

    let mut public_outputs: Vec<String> = serde_json::from_str(&output_json)?;
    let hyle_output = parse_noir_output(&mut public_outputs)?;

    Ok(hyle_output)
}

fn parse_noir_output(vector: &mut Vec<String>) -> Result<HyleOutput, Error> {
    let version = u32::from_str_radix(vector.remove(0).strip_prefix("0x").unwrap(), 16)?;
    let initial_state = parse_array(vector)?;
    let next_state = parse_array(vector)?;
    let identity = parse_string(vector)?;
    let tx_hash = parse_string(vector)?;
    let index = u32::from_str_radix(vector.remove(0).strip_prefix("0x").unwrap(), 16)?;
    let blobs = parse_blobs(vector)?;
    let success = u32::from_str_radix(vector.remove(0).strip_prefix("0x").unwrap(), 16)? == 1;

    Ok(HyleOutput {
        version,
        initial_state: StateDigest(initial_state),
        next_state: StateDigest(next_state),
        identity: identity.into(),
        tx_hash: TxHash(tx_hash),
        index: BlobIndex(index),
        blobs,
        success,
        program_outputs: vec![],
    })
}

fn parse_string(vector: &mut Vec<String>) -> Result<String, Error> {
    let length = usize::from_str_radix(vector.remove(0).strip_prefix("0x").unwrap(), 16)?;
    let mut resp = String::with_capacity(length);
    for _ in 0..length {
        let code = u32::from_str_radix(vector.remove(0).strip_prefix("0x").unwrap(), 16)?;
        let ch = std::char::from_u32(code)
            .ok_or_else(|| anyhow::anyhow!("Invalid char code: {}", code))?;
        resp.push(ch);
    }
    Ok(resp)
}

fn parse_array(vector: &mut Vec<String>) -> Result<Vec<u8>, Error> {
    let length = usize::from_str_radix(vector.remove(0).strip_prefix("0x").unwrap(), 16)?;
    let mut resp = Vec::with_capacity(length);
    for _ in 0..length {
        let num = u8::from_str_radix(vector.remove(0).strip_prefix("0x").unwrap(), 16)?;
        resp.push(num);
    }
    Ok(resp)
}

fn parse_blobs(vector: &mut Vec<String>) -> Result<Vec<u8>, Error> {
    let _blob_len = usize::from_str_radix(vector.remove(0).strip_prefix("0x").unwrap(), 16)?;
    // Arbitrary value that says that blobs field is size for only 10 elements
    let blob_data: Vec<String> = vector.drain(0..10.min(vector.len())).collect();
    let mut blob_data = VecDeque::from(blob_data);

    let blob_number = usize::from_str_radix(
        blob_data
            .pop_front()
            .ok_or_else(|| anyhow::anyhow!("Missing blob number"))?
            .strip_prefix("0x")
            .unwrap(),
        16,
    )?;
    let mut blobs = Vec::new();

    for _ in 0..blob_number {
        let blob_size = usize::from_str_radix(
            blob_data
                .pop_front()
                .ok_or_else(|| anyhow::anyhow!("Missing blob size"))?
                .strip_prefix("0x")
                .unwrap(),
            16,
        )?;

        for _ in 0..blob_size {
            let v = &blob_data
                .pop_front()
                .ok_or_else(|| anyhow::anyhow!("Missing blob data"))?;
            blobs.push(u8::from_str_radix(v.strip_prefix("0x").unwrap(), 16)?);
        }
    }

    Ok(blobs)
}

#[cfg(test)]
mod tests {
    use std::{fs::File, io::Read};

    use hydentity::Hydentity;
    use hyle_contract_sdk::{
        identity_provider::IdentityVerification, BlobIndex, HyleOutput, Identity, StateDigest,
        TxHash,
    };

    use super::{noir_proof_verifier, risc0_proof_verifier};

    fn load_file_as_bytes(path: &str) -> Vec<u8> {
        let mut file = File::open(path).expect("Failed to open file");
        let mut encoded_receipt = Vec::new();
        file.read_to_end(&mut encoded_receipt)
            .expect("Failed to read file content");
        encoded_receipt
    }

    #[test_log::test]
    fn test_risc0_proof_verifier() {
        std::env::set_var("RISC0_DEV_MODE", "1");
        let encoded_receipt =
            load_file_as_bytes("./tests/proofs/register.bob.hydentity.risc0.proof");

        let hydentity_program_id = include_str!("../../contracts/hydentity/hydentity.txt").trim();
        let image_id = hex::decode(hydentity_program_id).expect("Image id decoding failed");

        let result = risc0_proof_verifier(&encoded_receipt, &image_id);

        let mut next_state = Hydentity::default();
        next_state
            .register_identity("faucet.hydentity", "password")
            .unwrap();

        match result {
            Ok(outputs) => {
                assert_eq!(
                    outputs.program_outputs,
                    "Successfully registered identity for account: bob.hydentity"
                        .to_owned()
                        .as_bytes()
                        .to_vec()
                );
            }
            Err(e) => panic!("Risc0 verification failed: {:?}", e),
        }
    }

    /*
        For this test, the proof/vk and the output are obtained running this simple Noir code
        ```
            fn main(
            version: pub u32,
            initial_state_len: pub u32,
            initial_state: pub [u8; 4],
            next_state_len: pub u32,
            next_state: pub [u8; 4],
            identity_len: pub u8,
            identity: pub str<56>,
            tx_hash_len: pub u32,
            tx_hash: pub [u8; 0],
            index: pub u32,
            blobs_len: pub u32,
            blobs: pub [Field; 10],
            success: pub bool
            ) {}
        ```
        With the values:
        ```
            version = 1
            blobs = [3, 1, 1, 2, 1, 1, 2, 1, 1, 0]
            initial_state_len = 4
            initial_state = [0, 0, 0, 0]
            next_state_len = 4
            next_state = [0, 0, 0, 0]
            identity_len = 56
            identity = "3f368bf90c71946fc7b0cde9161ace42985d235f.ecdsa_secp256r1"
            tx_hash_len = 0
            tx_hash = []
            blobs_len = 9
            index = 0
            success = 1
        ```
    */
    #[test_log::test]
    fn test_noir_proof_verifier() {
        let noir_proof = load_file_as_bytes("./tests/proofs/webauthn.noir.proof");
        let image_id = load_file_as_bytes("./tests/proofs/webauthn.noir.vk");

        let result = noir_proof_verifier(&noir_proof, &image_id);
        match result {
            Ok(outputs) => {
                assert_eq!(
                    outputs,
                    HyleOutput {
                        version: 1,
                        initial_state: StateDigest(vec![0, 0, 0, 0]),
                        next_state: StateDigest(vec![0, 0, 0, 0]),
                        identity: Identity(
                            "3f368bf90c71946fc7b0cde9161ace42985d235f.ecdsa_secp256r1".to_owned()
                        ),
                        tx_hash: TxHash("".to_owned()),
                        index: BlobIndex(0),
                        blobs: vec![1, 1, 1, 1, 1],
                        success: true,
                        program_outputs: vec![]
                    }
                );
            }
            Err(e) => panic!("Noir verification failed: {:?}", e),
        }
    }
}
