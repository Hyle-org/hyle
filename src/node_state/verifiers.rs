use anyhow::{bail, Context, Error};
use borsh::from_slice;
use risc0_zkvm::sha::Digest;
use sp1_sdk::{ProverClient, SP1ProofWithPublicValues, SP1VerifyingKey};

use crate::model::ProofTransaction;
use hyle_contract_sdk::HyleOutput;

pub fn verify_proof(
    tx: &ProofTransaction,
    verifier: &str,
    program_id: &[u8],
) -> Result<HyleOutput, Error> {
    // TODO: remove test
    match verifier {
        "test" => Ok(serde_json::from_slice(&tx.proof.to_bytes()?)?),
        "risc0" => risc0_proof_verifier(&tx.proof.to_bytes()?, program_id),
        "sp1" => sp1_proof_verifier(&tx.proof.to_bytes()?, program_id),
        _ => bail!("{} verifier not implemented yet", verifier),
    }
}

pub fn risc0_proof_verifier(encoded_receipt: &[u8], image_id: &[u8]) -> Result<HyleOutput, Error> {
    let receipt = from_slice::<risc0_zkvm::Receipt>(encoded_receipt)
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

    use hydentity::Hydentity;
    use hyle_contract_sdk::{identity_provider::IdentityVerification, StateDigest};
    use serde_json::json;

    use super::risc0_proof_verifier;

    fn load_encoded_receipt_from_file(path: &str) -> Vec<u8> {
        let mut file = File::open(path).expect("Failed to open proof file");
        let mut encoded_receipt = Vec::new();
        file.read_to_end(&mut encoded_receipt)
            .expect("Failed to read file content");
        encoded_receipt
    }

    #[test]
    fn test_risc0_proof_verifier() {
        std::env::set_var("RISC0_DEV_MODE", "1");
        let encoded_receipt =
            load_encoded_receipt_from_file("./tests/proofs/register.bob.hydentity.risc0.proof");

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

    #[test_log::test(test)]
    fn test_sp1_proof_verifier() {
        let encoded_proof = load_encoded_receipt_from_file("./tests/proofs/sp1_basic_proof.bin");
        let verification_key = serde_json::to_string(&json!({
            "vk":{
                "commit":{
                    "value":[575007420,1261203209,1098152299,719575249,1753499905,1551016589,1342492089,1942151841],"_marker":null},
                    "pc_start":2105004,
                    "chip_information":[["MemoryProgram",{"log_n":19,"shift":1},{"width":6,"height":524288}],["Program",{"log_n":19,"shift":1},{"width":37,"height":524288}],["Byte",{"log_n":16,"shift":1},{"width":11,"height":65536}]],
                    "chip_ordering":{"Byte":2,"MemoryProgram":0,"Program":1
                }
            }
        })).unwrap();

        let result =
            super::sp1_proof_verifier(&encoded_proof, verification_key.as_bytes()).unwrap();

        assert_eq!(
            &result.initial_state,
            &StateDigest(29u32.to_le_bytes().to_vec())
        );
        assert_eq!(
            &result.next_state,
            &StateDigest(vec![
                0x1d, 0x00, 0x00, 0x00, 0xb5, 0xd8, 0x07, 0x00, 0x28, 0xb2, 0x0c, 0x00
            ])
        );
    }
}
