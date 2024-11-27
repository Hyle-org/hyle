use anyhow::{bail, Error};
use borsh::from_slice;
use cairo_platinum_prover::air::verify_cairo_proof;
use risc0_zkvm::sha::Digest;
use stark_platinum_prover::proof::options::{ProofOptions, SecurityLevel};

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
        "cairo" => cairo_proof_verifier(&tx.proof.to_bytes()?),
        "risc0" => risc0_proof_verifier(&tx.proof.to_bytes()?, program_id),
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

    tracing::info!("🦄🦄 receipt.journal {:?} ", receipt.journal);
    let hyle_output = match receipt.journal.decode::<HyleOutput>() {
        Ok(v) => v,
        Err(e) => bail!("Failed to extract HyleOuput from Risc0's journal: {}", e),
    };

    // // TODO: allow multiple outputs when verifying
    Ok(hyle_output)
}

#[cfg(test)]
mod tests {
    use std::{fs::File, io::Read};

    use hydentity::Hydentity;
    use hyle_contract_sdk::{
        flatten_blobs,
        identity_provider::{IdentityAction, IdentityVerification},
        BlobIndex, ContractName, Digestable, HyleOutput, Identity, TxHash,
    };

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
        let encoded_receipt =
            load_encoded_receipt_from_file("./tests/proofs/register.hydentity.risc0.proof");

        let hydentity_program_id = include_str!("../../contracts/hydentity/hydentity.txt").trim();
        let image_id = hex::decode(hydentity_program_id).expect("Image id decoding failed");

        let result = risc0_proof_verifier(&encoded_receipt, &image_id);

        let mut next_state = Hydentity::default();
        next_state
            .register_identity("faucet.hydentity", "password")
            .unwrap();
        let next_state = next_state.as_digest();

        let blob_data = IdentityAction::RegisterIdentity {
            account: "faucet.hydentity".to_string(),
        };
        let blobs = vec![(blob_data.as_blob(ContractName("hydentity".to_owned()), None, None))];

        match result {
            Ok(outputs) => {
                assert_eq!(
                    outputs,
                    HyleOutput {
                        version: 1,
                        initial_state: Hydentity::default().as_digest(),
                        next_state,
                        identity: Identity("faucet.hydentity".to_owned()),
                        tx_hash: TxHash("".to_owned()),
                        index: BlobIndex(0),
                        blobs: flatten_blobs(&blobs),
                        success: true,
                        program_outputs:
                            "Successfully registered identity for account: faucet.hydentity"
                                .to_owned()
                                .as_bytes()
                                .to_vec()
                    }
                );
            }
            Err(e) => panic!("Risc0 verification failed: {:?}", e),
        }
    }
}
