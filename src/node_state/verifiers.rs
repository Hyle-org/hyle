use anyhow::{bail, Error};
use borsh::from_slice;
use cairo_platinum_prover::air::verify_cairo_proof;
use risc0_zkvm::sha::Digest;
use stark_platinum_prover::proof::options::{ProofOptions, SecurityLevel};
use tracing::info;

use crate::model::ProofTransaction;
use hyle_contract_sdk::{HyleOutput, Identity, StateDigest};

pub fn verify_proof(proof: &[u8], verifier: &str, program_id: &[u8]) -> Result<HyleOutput, Error> {
    // TODO: remove test
    match verifier {
        //"test" => {
        //    let tx_hash = tx.blobs_references.first().unwrap().blob_tx_hash.clone();
        //    let index = tx.blobs_references.first().unwrap().blob_index.clone();
        //    Ok(HyleOutput {
        //        version: 1,
        //        initial_state: StateDigest(vec![0, 1, 2, 3]),
        //        next_state: StateDigest(vec![4, 5, 6]),
        //        identity: Identity("test".to_string()),
        //        tx_hash,
        //        index,
        //        blobs: vec![0, 1, 2, 3, 0, 1, 2, 3],
        //        success: true,
        //        program_outputs: vec![],
        //    })
        //}
        "cairo" => cairo_proof_verifier(proof),
        "risc0" => risc0_proof_verifier(proof, program_id),
        _ => bail!("{} verifier not implemented yet", verifier),
    }
}

pub fn verify_recursion_proof(proof: &[u8], verifier: &str) -> Result<Vec<HyleOutput>, Error> {
    // TODO: remove test
    match verifier {
        //"test" => Ok(tx
        //    .blobs_references
        //    .iter()
        //    .map(|blob_ref| HyleOutput {
        //        version: 1,
        //        initial_state: StateDigest(vec![0, 1, 2, 3]),
        //        next_state: StateDigest(vec![4, 5, 6]),
        //        identity: Identity("test".to_string()),
        //        tx_hash: blob_ref.blob_tx_hash.clone(),
        //        index: blob_ref.blob_index.clone(),
        //        blobs: vec![0, 1, 2, 3, 0, 1, 2, 3],
        //        success: true,
        //        program_outputs: vec![],
        //    })
        //    .collect()),
        _ => bail!("{} recursion verifier not implemented yet", verifier),
    }
}

pub fn cairo_proof_verifier(proof: &[u8]) -> Result<HyleOutput, Error> {
    let proof_options = ProofOptions::new_secure(SecurityLevel::Conjecturable100Bits, 3);

    let mut bytes = proof;
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

    info!("🔎 Verifying Risc0 proof");

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

    use hyle_contract_sdk::{BlobIndex, HyleOutput, Identity, StateDigest, TxHash};

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
        let encoded_receipt = load_encoded_receipt_from_file("./tests/proofs/erc20.risc0.proof");

        let image_id =
            hex::decode("0f0e89496853ab498a5eda2d06ced45909faf490776c8121063df9066bbb9ea4")
                .expect("Image id decoding failed");

        let result = risc0_proof_verifier(&encoded_receipt, &image_id);

        match result {
            Ok(outputs) => {
                assert_eq!(
                    outputs,
                    HyleOutput {
                        version: 1,
                        initial_state: StateDigest(vec![
                            237, 40, 107, 60, 57, 178, 248, 111, 156, 232, 107, 188, 53, 69, 95,
                            231, 232, 247, 179, 249, 104, 59, 167, 110, 11, 204, 99, 126, 181, 96,
                            47, 61
                        ]),
                        next_state: StateDigest(vec![
                            154, 65, 139, 95, 54, 114, 201, 168, 66, 153, 34, 153, 43, 237, 17,
                            198, 0, 39, 64, 81, 204, 183, 209, 41, 84, 147, 193, 217, 48, 42, 213,
                            57
                        ]),
                        identity: Identity("max".to_owned()),
                        tx_hash: TxHash("01".to_owned()),
                        index: BlobIndex(0),
                        blobs: vec![1, 3, 109, 97, 120, 27],
                        success: true,
                        program_outputs: "Minted 27 to max".to_owned().as_bytes().to_vec()
                    }
                );
            }
            Err(e) => panic!("Risc0 verification failed: {:?}", e),
        }
    }
}
