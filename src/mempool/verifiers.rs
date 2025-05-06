use anyhow::{Context, Result};
use hyle_contract_sdk::{HyleOutput, ProgramId, Verifier};
use hyle_model::ProofData;

pub fn verify_proof(
    proof: &ProofData,
    verifier: &Verifier,
    #[allow(unused_variables)] program_id: &ProgramId,
) -> Result<Vec<HyleOutput>> {
    let hyle_outputs = match verifier.0.as_str() {
        // TODO: add #[cfg(test)]
        "test" => borsh::from_slice::<Vec<HyleOutput>>(&proof.0).context("parsing test proof"),
        #[cfg(test)]
        "test-slow" => {
            tracing::info!("Sleeping for 2 seconds to simulate a slow verifier");
            std::thread::sleep(std::time::Duration::from_secs(2));
            tracing::info!("Woke up from sleep");
            Ok(serde_json::from_slice(&proof.0)?)
        }
        _ => hyle_verifiers::verify(verifier, proof, program_id),
    }?;
    hyle_outputs.iter().for_each(|hyle_output| {
        tracing::debug!(
            "🔎 {}",
            std::str::from_utf8(&hyle_output.program_outputs)
                .map(|o| format!("Program outputs: {o}"))
                .unwrap_or("Invalid UTF-8".to_string())
        );
    });

    Ok(hyle_outputs)
}

pub fn verify_recursive_proof(
    proof: &ProofData,
    verifier: &Verifier,
    program_id: &ProgramId,
) -> Result<(Vec<ProgramId>, Vec<HyleOutput>)> {
    let outputs: (Vec<ProgramId>, Vec<HyleOutput>) = match verifier.0.as_str() {
        #[cfg(feature = "risc0")]
        hyle_model::verifiers::RISC0_1 => {
            hyle_verifiers::risc0_1::verify_recursive(proof, program_id)
        }
        _ => Err(anyhow::anyhow!(
            "{} recursive verifier not implemented yet (or feature disabled)",
            verifier
        )),
    }?;
    outputs.1.iter().for_each(|hyle_output| {
        tracing::debug!(
            "🔎 {}",
            std::str::from_utf8(&hyle_output.program_outputs)
                .map(|o| format!("Program outputs: {o}"))
                .unwrap_or("Invalid UTF-8".to_string())
        );
    });

    Ok(outputs)
}
