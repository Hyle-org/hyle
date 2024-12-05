use anyhow::{bail, Error};
use borsh::from_slice;
use risc0_zkvm::sha::Digest;

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
        _ => bail!("{} verifier not implemented yet", verifier),
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

#[cfg(test)]
mod tests {
    use std::{fs::File, io::Read};

    use hydentity::Hydentity;
    use hyle_contract_sdk::identity_provider::IdentityVerification;

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
}
