#![warn(unused_crate_dependencies)]

use std::fmt::Write;
use std::io::Read;

use anyhow::{bail, Context, Error};
use hyle_model::{HyleOutput, ProgramId};
use rand::Rng;

#[cfg(feature = "sp1")]
use sp1_sdk::{ProverClient, SP1ProofWithPublicValues, SP1VerifyingKey};
use tracing::debug;

pub mod noir_utils;

pub mod risc0 {
    pub use risc0_zkvm::serde::from_slice;
}

pub fn risc0_proof_verifier(
    encoded_receipt: &[u8],
    image_id: &[u8],
) -> Result<risc0_zkvm::Journal, Error> {
    let receipt = borsh::from_slice::<risc0_zkvm::Receipt>(encoded_receipt)
        .context("Error while decoding Risc0 proof's receipt")?;

    let image_bytes: risc0_zkvm::sha::Digest =
        image_id.try_into().context("Invalid Risc0 image ID")?;

    receipt
        .verify(image_bytes)
        .context("Risc0 proof verification failed")?;

    tracing::info!("✅ Risc0 proof verified.");

    Ok(receipt.journal)
}

/// At present, we are using binary to facilitate the integration of the Noir verifier.
/// This is not meant to be a permanent solution.
pub fn noir_proof_verifier(proof: &[u8], image_id: &[u8]) -> Result<Vec<HyleOutput>, Error> {
    // Define a struct with Drop implementation for cleanup
    struct TempFiles {
        proof_path: String,
        vk_path: String,
    }

    impl Drop for TempFiles {
        fn drop(&mut self) {
            if std::env::var("HYLE_KEEP_NOIR_TMP_FILES").unwrap_or_else(|_| "false".to_string())
                != "true"
            {
                let _ = std::fs::remove_file(&self.proof_path);
                let _ = std::fs::remove_file(&self.vk_path);
            }
        }
    }

    let mut rng = rand::rng();
    let salt: [u8; 16] = rng.random();
    let mut salt_hex = String::with_capacity(salt.len() * 2);
    for b in &salt {
        write!(salt_hex, "{:02x}", b).unwrap();
    }

    // Create the temp files struct which will auto-clean on function exit
    let temp_files = TempFiles {
        proof_path: format!("/tmp/noir-proof-{salt_hex}"),
        vk_path: format!("/tmp/noir-vk-{salt_hex}"),
    };

    // Write proof and publicKey to files
    std::fs::write(&temp_files.proof_path, proof)?;
    std::fs::write(&temp_files.vk_path, image_id)?;

    debug!(
        "Proof path: {} VK path: {}",
        temp_files.proof_path, temp_files.vk_path
    );

    // Verifying proof
    let verification_output = std::process::Command::new("bb")
        .arg("verify")
        .arg("-p")
        .arg(&temp_files.proof_path)
        .arg("-k")
        .arg(&temp_files.vk_path)
        .output()?;

    if !verification_output.status.success() {
        bail!(
            "Noir proof verification failed: {}",
            String::from_utf8_lossy(&verification_output.stderr)
        );
    }

    // Extracting outputs
    let mut file =
        std::fs::File::open(&temp_files.proof_path).context("Failed to open proof file")?;
    let mut proof = Vec::new();
    file.read_to_end(&mut proof)
        .context("Failed to read proof file content")?;

    // TODO: support multi-output proofs.
    let hyle_output = crate::noir_utils::parse_noir_output(&proof)?;

    tracing::info!("✅ Noir proof verified.");

    Ok(vec![hyle_output])
    // temp_files is automatically dropped here, cleaning up all files
}

/// The following environment variables are used to configure the prover:
/// - `SP1_PROVER`: The type of prover to use. Must be one of `mock`, `local`, `cuda`, or `network`.
#[cfg(feature = "sp1")]
pub fn sp1_proof_verifier(
    proof_bin: &[u8],
    verification_key: &[u8],
) -> Result<Vec<HyleOutput>, Error> {
    // Setup the prover client.
    let client = ProverClient::from_env();

    let proof: SP1ProofWithPublicValues =
        bincode::deserialize(proof_bin).context("Error while decoding SP1 proof.")?;

    // Deserialize verification key from JSON
    let vk: SP1VerifyingKey =
        serde_json::from_slice(verification_key).context("Invalid SP1 image ID")?;

    // Verify the proof.
    client
        .verify(&proof, &vk)
        .context("SP1 proof verification failed")?;

    // TODO: support multi-output proofs.
    let hyle_output = borsh::from_slice::<HyleOutput>(proof.public_values.as_slice())
        .context("Failed to extract HyleOuput from SP1 proof")?;

    tracing::info!("✅ SP1 proof verified.",);

    Ok(vec![hyle_output])
}

pub fn validate_risc0_program_id(program_id: &ProgramId) -> Result<(), Error> {
    std::convert::TryInto::<risc0_zkvm::sha::Digest>::try_into(program_id.0.as_slice())
        .map_err(|e| anyhow::anyhow!("Invalid Risc0 image ID: {}", e))?;
    Ok(())
}

#[cfg(feature = "sp1")]
pub fn validate_sp1_program_id(program_id: &ProgramId) -> Result<(), Error> {
    serde_json::from_slice::<SP1VerifyingKey>(program_id.0.as_slice())
        .map_err(|e| anyhow::anyhow!("Invalid SP1 image ID: {}", e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs::File, io::Read};

    use hyle_model::{BlobIndex, HyleOutput, Identity, ProgramId, StateCommitment, TxHash};

    use crate::validate_risc0_program_id;

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
                        initial_state: StateCommitment(vec![0, 0, 0, 0]),
                        next_state: StateCommitment(vec![0, 0, 0, 0]),
                        identity: Identity(
                            "3f368bf90c71946fc7b0cde9161ace42985d235f.ecdsa_secp256r1".to_owned()
                        ),
                        index: BlobIndex(0),
                        blobs: vec![(BlobIndex(0), vec![1, 1, 1, 1, 1])],
                        tx_blob_count: 1,
                        success: true,
                        tx_hash: TxHash::default(), // TODO
                        tx_ctx: None,
                        onchain_effects: vec![],
                        program_outputs: vec![]
                    }]
                );
            }
            Err(e) => panic!("Noir verification failed: {:?}", e),
        }
    }

    #[test]
    fn test_check_risc0_program_id() {
        let valid_program_id = ProgramId(vec![0; 32]); // Assuming a valid 32-byte ID
        let invalid_program_id = ProgramId(vec![0; 31]); // Invalid length

        assert!(validate_risc0_program_id(&valid_program_id).is_ok());
        assert!(validate_risc0_program_id(&invalid_program_id).is_err());
    }
}
