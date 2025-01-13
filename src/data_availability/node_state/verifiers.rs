use std::fmt::Write;
use std::io::Read;

use anyhow::{bail, Context, Error};
use rand::Rng;
use risc0_recursion::{Risc0Journal, Risc0ProgramId};
use risc0_zkvm::sha::Digest;
use sp1_sdk::{ProverClient, SP1ProofWithPublicValues, SP1VerifyingKey};

use hyle_contract_sdk::{HyleOutput, Identity, ProgramId, StateDigest, Verifier};

use crate::utils::crypto::{BlstCrypto, Signed, ValidatorSignature};

pub fn verify_proof(
    proof: &[u8],
    verifier: &Verifier,
    program_id: &ProgramId,
) -> Result<Vec<HyleOutput>, Error> {
    let hyle_outputs = match verifier.0.as_str() {
        // TODO: add #[cfg(test)]
        "test" => Ok(serde_json::from_slice(proof)?),
        #[cfg(test)]
        "test-slow" => {
            tracing::info!("Sleeping for 2 seconds to simulate a slow verifier");
            std::thread::sleep(std::time::Duration::from_secs(2));
            tracing::info!("Woke up from sleep");
            Ok(serde_json::from_slice(proof)?)
        }
        "risc0" => {
            let journal = risc0_proof_verifier(proof, &program_id.0)?;
            // First try to decode it as a single HyleOutput
            Ok(match journal.decode::<HyleOutput>() {
                Ok(ho) => vec![ho],
                Err(_) => {
                    let hyle_output = journal
                        .decode::<Vec<Vec<u8>>>()
                        .context("Failed to extract HyleOuput from Risc0's journal")?;

                    // Doesn't actually work to just deserialize in one go.
                    hyle_output
                        .iter()
                        .map(|o| risc0_zkvm::serde::from_slice::<HyleOutput, _>(o))
                        .collect::<Result<Vec<_>, _>>()
                        .context("Failed to decode HyleOutput")?
                }
            })
        }
        "noir" => noir_proof_verifier(proof, &program_id.0),
        "sp1" => sp1_proof_verifier(proof, &program_id.0),
        "native" => native_verifier(proof, &program_id),
        _ => bail!("{} recursive verifier not implemented yet", verifier),
    }?;
    hyle_outputs.iter().for_each(|hyle_output| {
        tracing::info!(
            "🔎 {}",
            std::str::from_utf8(&hyle_output.program_outputs)
                .map(|o| format!("Program outputs: {o}"))
                .unwrap_or("Invalid UTF-8".to_string())
        );
    });

    Ok(hyle_outputs)
}

pub fn verify_recursive_proof(
    proof: &[u8],
    verifier: &Verifier,
    program_id: &ProgramId,
) -> Result<(Vec<ProgramId>, Vec<HyleOutput>), Error> {
    let outputs = match verifier.0.as_str() {
        "risc0" => {
            let journal = risc0_proof_verifier(proof, &program_id.0)?;
            let mut output = journal
                .decode::<Vec<(Risc0ProgramId, Risc0Journal)>>()
                .context("Failed to extract HyleOuput from Risc0's journal")?;

            // Doesn't actually work to just deserialize in one go.
            output
                .drain(..)
                .map(|o| {
                    risc0_zkvm::serde::from_slice::<HyleOutput, _>(&o.1)
                        .map(|h| (ProgramId(o.0.to_vec()), h))
                })
                .collect::<Result<(Vec<_>, Vec<_>), _>>()
                .context("Failed to decode HyleOutput")
        }
        _ => bail!("{} recursive verifier not implemented yet", verifier),
    }?;
    outputs.1.iter().for_each(|hyle_output| {
        tracing::info!(
            "🔎 {}",
            std::str::from_utf8(&hyle_output.program_outputs)
                .map(|o| format!("Program outputs: {o}"))
                .unwrap_or("Invalid UTF-8".to_string())
        );
    });

    Ok(outputs)
}

pub fn risc0_proof_verifier(
    encoded_receipt: &[u8],
    image_id: &[u8],
) -> Result<risc0_zkvm::Journal, Error> {
    let receipt = borsh::from_slice::<risc0_zkvm::Receipt>(encoded_receipt)
        .context("Error while decoding Risc0 proof's receipt")?;

    let image_bytes: Digest = image_id.try_into().context("Invalid Risc0 image ID")?;

    receipt
        .verify(image_bytes)
        .context("Risc0 proof verification failed")?;

    tracing::info!("✅ Risc0 proof verified.");

    Ok(receipt.journal)
}

/// At present, we are using binary to facilitate the integration of the Noir verifier.
/// This is not meant to be a permanent solution.
pub fn noir_proof_verifier(proof: &[u8], image_id: &[u8]) -> Result<Vec<HyleOutput>, Error> {
    let mut rng = rand::thread_rng();
    let salt: [u8; 16] = rng.gen();
    let mut salt_hex = String::with_capacity(salt.len() * 2);
    for b in &salt {
        write!(salt_hex, "{:02x}", b).unwrap();
    }

    let proof_path = &format!("/tmp/noir-proof-{salt_hex}");
    let vk_path = &format!("/tmp/noir-vk-{salt_hex}");
    let output_path = &format!("/tmp/noir-output-{salt_hex}");

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
    // TODO: support multi-output proofs.
    let hyle_output = crate::utils::noir_utils::parse_noir_output(&mut public_outputs)?;

    // Delete proof_path, vk_path, output_path
    let _ = std::fs::remove_file(proof_path);
    let _ = std::fs::remove_file(vk_path);
    let _ = std::fs::remove_file(output_path);
    Ok(vec![hyle_output])
}

/// The following environment variables are used to configure the prover:
/// - `SP1_PROVER`: The type of prover to use. Must be one of `mock`, `local`, `cuda`, or `network`.
pub fn sp1_proof_verifier(
    proof_bin: &[u8],
    verification_key: &[u8],
) -> Result<Vec<HyleOutput>, Error> {
    // Setup the prover client.
    let client = ProverClient::from_env();

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

    // TODO: support multi-output proofs.
    let (hyle_output, _) = bincode::decode_from_slice::<HyleOutput, _>(
        proof.0.public_values.as_slice(),
        bincode::config::legacy().with_fixed_int_encoding(),
    )
    .context("Failed to extract HyleOuput from SP1 proof")?;

    tracing::info!("✅ SP1 proof verified.",);

    Ok(vec![hyle_output])
}

#[derive(Debug, bincode::Encode, bincode::Decode)]
struct NativeProof {
    tx_hash: hyle_contract_sdk::TxHash,
    index: hyle_contract_sdk::BlobIndex,
    blobs: Vec<hyle_contract_sdk::Blob>,
}

#[derive(Debug, bincode::Encode, bincode::Decode)]
struct BlstSignatureBlob {
    pub identity: Identity,
    pub data: Vec<u8>,
    /// Signature for contatenated data + identity.as_bytes()
    pub signature: Vec<u8>,
    pub public_key: Vec<u8>,
}

pub fn native_verifier(proof_bin: &[u8], program_id: &ProgramId) -> Result<Vec<HyleOutput>, Error> {
    let program = String::from_utf8(program_id.0.clone())?;
    let (proof, _) =
        bincode::decode_from_slice::<NativeProof, _>(proof_bin, bincode::config::standard())?;

    let NativeProof {
        tx_hash,
        index,
        blobs,
    } = proof;
    let blob = blobs
        .get(index.0)
        .ok_or(anyhow::anyhow!("Invalid blob index"))?;
    let blobs = hyle_contract_sdk::flatten_blobs(&blobs);

    let (identity, success) = match program.as_str() {
        "blst" => {
            let (blob, _) = bincode::decode_from_slice::<BlstSignatureBlob, _>(
                &blob.data.0,
                bincode::config::standard(),
            )?;

            let msg = vec![blob.data, blob.identity.0.as_bytes().to_vec()].concat();
            let msg = Signed {
                msg,
                signature: ValidatorSignature {
                    signature: crate::utils::crypto::Signature(blob.signature),
                    validator: staking::model::ValidatorPublicKey(blob.public_key),
                },
            };
            let success = BlstCrypto::verify(&msg)?;

            (blob.identity, success)
        }
        _ => bail!("Native verifier not implemented for program {}", program),
    };

    let output = HyleOutput {
        version: 1,
        initial_state: StateDigest::default(),
        next_state: StateDigest::default(),
        identity,
        tx_hash,
        index,
        blobs,
        success,
        program_outputs: vec![],
    };
    Ok(vec![output])
}

#[cfg(test)]
mod tests {
    use std::{fs::File, io::Read};

    use hyle_contract_sdk::{
        StateDigest, {BlobIndex, HyleOutput, Identity, TxHash},
    };

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
    #[ignore = "manual test"]
    #[test_log::test]
    fn test_noir_proof_verifier() {
        let noir_proof = load_file_as_bytes("./tests/proofs/webauthn.noir.proof");
        let image_id = load_file_as_bytes("./tests/proofs/webauthn.noir.vk");

        let result = noir_proof_verifier(&noir_proof, &image_id);
        match result {
            Ok(outputs) => {
                assert_eq!(
                    outputs,
                    vec![HyleOutput {
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
                    }]
                );
            }
            Err(e) => panic!("Noir verification failed: {:?}", e),
        }
    }
}
