use anyhow::{bail, Error};
use borsh::from_slice;
use cairo_platinum_prover::air::verify_cairo_proof;
use risc0_zkvm::sha::Digest;
use stark_platinum_prover::proof::options::{ProofOptions, SecurityLevel};

use crate::model::ProofTransaction;
use crate::model::{Identity, StateDigest};

use super::model::HyleOutput;

pub fn verify_proof(
    tx: &ProofTransaction,
    verifier: &str,
    program_id: &[u8],
) -> Result<Vec<HyleOutput>, Error> {
    // TODO: remove test
    match verifier {
        "test" => Ok(tx
            .blobs_references
            .iter()
            .map(|blob_ref| HyleOutput {
                version: 1,
                initial_state: StateDigest(vec![0, 1, 2, 3]),
                next_state: StateDigest(vec![4, 5, 6]),
                identity: Identity("test".to_string()),
                tx_hash: blob_ref.blob_tx_hash.clone(),
                index: blob_ref.blob_index.clone(),
                blobs: vec![0, 1, 2, 3, 0, 1, 2, 3],
                success: true,
            })
            .collect()),
        "cairo" => cairo_proof_verifier(&tx.proof),
        "risc0" => risc0_proof_verifier(&tx.proof, program_id),
        _ => bail!("{} verifier not implemented yet", verifier),
    }
}

pub fn cairo_proof_verifier(proof: &Vec<u8>) -> Result<Vec<HyleOutput>, Error> {
    let proof_options = ProofOptions::new_secure(SecurityLevel::Conjecturable100Bits, 3);

    let mut bytes = proof.as_slice();
    if bytes.len() < 8 {
        bail!("Cairo oroof is too short");
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

    let program_output = match bincode::serde::decode_from_slice::<Vec<HyleOutput>, _>(
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

pub fn risc0_proof_verifier(
    encoded_receipt: &[u8],
    image_id: &[u8],
) -> Result<Vec<HyleOutput>, Error> {
    let receipt = match from_slice::<risc0_zkvm::Receipt>(encoded_receipt) {
        Ok(v) => v,
        Err(e) => bail!(
            "Error while decoding Risc0 proof's receipt. Decode error: {}",
            e
        ),
    };

    let image_bytes: Digest = image_id.try_into().expect("Invalid Risc0 image ID");

    // On peut récuperer l'image ID depuis le Receipt et le comparer avec ce qu'on a pour ne pas lancer la vérification s'ils ne matchent pas
    // TODO: remove unwrap
    // let claim = receipt.claim().unwrap().value().unwrap();
    // println!("claim.pre.digest(): {:?}", claim.pre.digest());
    // asserteq!(claim.pre.digest(), image_bytes);

    receipt
        .verify(image_bytes)
        .expect("Risc0 proof verification failed");

    let hyle_output = receipt
        .journal
        .decode::<HyleOutput>()
        .expect("Failed to extract HyleOuput from Risc0's journal");

    // TODO: allow multiple outputs when verifying
    Ok(vec![hyle_output])
}

#[cfg(test)]
mod tests {
    use std::{fs::File, io::Read};

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
            hex::decode("a77b03e5db5b05ce9e05920e528a18985d40aecda607cbb808235c87f535f606")
                .expect("Image id decoding failed");

        let result = risc0_proof_verifier(&encoded_receipt, &image_id);

        match result {
            Ok(outputs) => {
                assert!(!outputs.is_empty(), "HyleOutput should not be empty");
            }
            Err(e) => panic!("Risc0 verification failed: {:?}", e),
        }
    }
}
