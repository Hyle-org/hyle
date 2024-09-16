use anyhow::{bail, Error};
use cairo_platinum_prover::air::verify_cairo_proof;
use stark_platinum_prover::proof::options::{ProofOptions, SecurityLevel};

use crate::model::ProofTransaction;

use super::model::HyleOutput;

pub fn verify_proof(tx: &ProofTransaction, verifier: &str) -> Result<Vec<HyleOutput>, Error> {
    match verifier {
        "cairo" => cairo_proof_verifier(&tx.proof),
        _ => bail!("{} verifier not implemented yet", verifier),
    }
}

pub fn cairo_proof_verifier(proof: &Vec<u8>) -> Result<Vec<HyleOutput>, Error> {
    let proof_options = ProofOptions::new_secure(SecurityLevel::Conjecturable100Bits, 3);

    let mut bytes = proof.as_slice();
    if bytes.len() < 8 {
        bail!("Proof is too short");
    }

    // Proof len was stored as an u32, 4u8 needs to be read
    let proof_len = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;

    bytes = &bytes[4..];
    if bytes.len() < proof_len {
        bail!("Proof is not correctly formed");
    }

    let proof = match bincode::serde::decode_from_slice(
        &bytes[0..proof_len],
        bincode::config::standard(),
    ) {
        Ok((proof, _)) => proof,
        Err(e) => {
            bail!("Error while decoding proof. Decode error: {}", e);
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
                    "Error while decoding proof's public input. Decode error: {}",
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
            bail!("Error while decoding proof's output. Decode error: {}", e);
        }
    };

    if verify_cairo_proof(&proof, &pub_inputs, &proof_options) {
        return Ok(program_output);
    } else {
        bail!("Proof verification failed.");
    }
}
