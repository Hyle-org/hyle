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
