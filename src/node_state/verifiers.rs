use std::{collections::VecDeque, io::Read};

use anyhow::{bail, Context, Error};
use rand::Rng;
use risc0_zkvm::sha::Digest;
use sp1_sdk::{ProverClient, SP1ProofWithPublicValues, SP1VerifyingKey};

use hyle_contract_sdk::{BlobIndex, HyleOutput, StateDigest, TxHash};

pub fn verify_proof(proof: &[u8], verifier: &str, program_id: &[u8]) -> Result<HyleOutput, Error> {
    // TODO: remove test
    match verifier {
        "test" => Ok(serde_json::from_slice(proof)?),
        "risc0" => risc0_proof_verifier(proof, program_id),
        "noir" => noir_proof_verifier(proof, program_id),
        "sp1" => sp1_proof_verifier(proof, program_id),
        _ => bail!("{} verifier not implemented yet", verifier),
    }
}

pub fn risc0_proof_verifier(encoded_receipt: &[u8], image_id: &[u8]) -> Result<HyleOutput, Error> {
    let receipt = borsh::from_slice::<risc0_zkvm::Receipt>(encoded_receipt)
        .context("Error while decoding Risc0 proof's receipt")?;

    let image_bytes: Digest = image_id.try_into().context("Invalid Risc0 image ID")?;

    receipt
        .verify(image_bytes)
        .context("Risc0 proof verification failed")?;

    let hyle_output = receipt
        .journal
        .decode::<HyleOutput>()
        .context("Failed to extract HyleOuput from Risc0's journal")?;

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

pub fn sp1_proof_verifier(proof_bin: &[u8], verification_key: &[u8]) -> Result<HyleOutput, Error> {
    // Setup the prover client.
    let client = ProverClient::new();

    let (proof, _) =
        bincode::decode_from_slice::<bincode::serde::Compat<SP1ProofWithPublicValues>, _>(
            proof_bin,
            bincode::config::legacy().with_fixed_int_encoding(),
        )
        .context("Error while decoding SP1 proof.")?;

    // Deserialize verification key from JSON
    let vk: SP1VerifyingKey =
        serde_json::from_slice(verification_key).context("Invalid SP1 image ID")?;

    // Verify the proof.
    client
        .verify(&proof.0, &vk)
        .context("SP1 proof verification failed")?;

    let (hyle_output, _) = bincode::decode_from_slice::<HyleOutput, _>(
        proof.0.public_values.as_slice(),
        bincode::config::legacy().with_fixed_int_encoding(),
    )
    .context("Failed to extract HyleOuput from SP1 proof")?;

    tracing::info!(
        "✅ SP1 proof verified. {}",
        std::str::from_utf8(&hyle_output.program_outputs)
            .map(|o| format!("Program outputs: {o}"))
            .unwrap_or("Invalid UTF-8".to_string())
    );

    Ok(hyle_output)
}

#[cfg(test)]
mod tests {
    use std::{fs::File, io::Read};

    use hyle_contract_sdk::{
        StateDigest, {BlobIndex, HyleOutput, Identity, TxHash},
    };
    use serde_json::json;

    use super::noir_proof_verifier;

    fn load_file_as_bytes(path: &str) -> Vec<u8> {
        let mut file = File::open(path).expect("Failed to open file");
        let mut encoded_receipt = Vec::new();
        file.read_to_end(&mut encoded_receipt)
            .expect("Failed to read file content");
        encoded_receipt
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
