use anyhow::{Context, Result};
use hyle_model::{Identity, ProofData, Signed, ValidatorSignature};
use sha3::Digest;

use hyle_contract_sdk::{Blob, BlobIndex, HyleOutput, ProgramId, StateDigest, TxHash, Verifier};

use hyle_verifiers::{noir_proof_verifier, risc0_proof_verifier, validate_risc0_program_id};

use crate::{
    model::verifiers::{BlstSignatureBlob, NativeVerifiers, ShaBlob},
    utils::crypto::BlstCrypto,
};

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
        "risc0" => {
            let journal = risc0_proof_verifier(&proof.0, &program_id.0)?;
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
                        .map(|o| hyle_verifiers::risc0::from_slice::<HyleOutput, _>(o))
                        .collect::<Result<Vec<_>, _>>()
                        .context("Failed to decode HyleOutput")?
                }
            })
        }
        "noir" => noir_proof_verifier(&proof.0, &program_id.0),
        #[cfg(feature = "sp1")]
        "sp1" => hyle_verifiers::sp1_proof_verifier(&proof.0, &program_id.0),
        _ => Err(anyhow::anyhow!("{} verifier not implemented yet", verifier)),
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
    use risc0_recursion::{Risc0Journal, Risc0ProgramId};

    let outputs = match verifier.0.as_str() {
        "risc0" => {
            let journal = risc0_proof_verifier(&proof.0, &program_id.0)?;
            let mut output = journal
                .decode::<Vec<(Risc0ProgramId, Risc0Journal)>>()
                .context("Failed to extract HyleOuput from Risc0's journal")?;

            // Doesn't actually work to just deserialize in one go.
            output
                .drain(..)
                .map(|o| {
                    hyle_verifiers::risc0::from_slice::<HyleOutput, _>(&o.1)
                        .map(|h| (ProgramId(o.0.to_vec()), h))
                })
                .collect::<Result<(Vec<_>, Vec<_>), _>>()
                .context("Failed to decode HyleOutput")
        }
        _ => Err(anyhow::anyhow!(
            "{} recursive verifier not implemented yet",
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

pub fn verify_native(
    tx_hash: TxHash,
    index: BlobIndex,
    blobs: &[Blob],
    verifier: NativeVerifiers,
) -> HyleOutput {
    #[allow(clippy::expect_used, reason = "Logic error in the code")]
    let blob = blobs.get(index.0).expect("Invalid blob index");
    let blobs = hyle_contract_sdk::flatten_blobs(blobs);

    let (identity, success) = match verify_native_impl(blob, verifier) {
        Ok((identity, success)) => (identity, success),
        Err(e) => {
            tracing::trace!("Native blob verification failed: {:?}", e);
            (Identity::default(), false)
        }
    };

    if success {
        tracing::info!("✅ Native blob verified on {tx_hash}:{index}");
    } else {
        tracing::info!("❌ Native blob verification failed on {tx_hash}:{index}.");
    }

    HyleOutput {
        version: 1,
        initial_state: StateDigest::default(),
        next_state: StateDigest::default(),
        identity,
        index,
        blobs,
        success,
        tx_hash,
        tx_ctx: None,
        registered_contracts: vec![],
        program_outputs: vec![],
    }
}

pub fn verify_native_impl(
    blob: &Blob,
    verifier: NativeVerifiers,
) -> anyhow::Result<(Identity, bool)> {
    match verifier {
        NativeVerifiers::Blst => {
            let blob = borsh::from_slice::<BlstSignatureBlob>(&blob.data.0)?;

            let msg = [blob.data, blob.identity.0.as_bytes().to_vec()].concat();
            // TODO: refacto BlstCrypto to avoid using ValidatorPublicKey here
            let msg = Signed {
                msg,
                signature: ValidatorSignature {
                    signature: crate::model::Signature(blob.signature),
                    validator: crate::model::ValidatorPublicKey(blob.public_key),
                },
            };
            Ok((blob.identity, BlstCrypto::verify(&msg)?))
        }
        NativeVerifiers::Sha3_256 => {
            let blob = borsh::from_slice::<ShaBlob>(&blob.data.0)?;

            let mut hasher = sha3::Sha3_256::new();
            hasher.update(blob.data);
            let res = hasher.finalize().to_vec();

            Ok((blob.identity, res == blob.sha))
        }
    }
}

pub fn validate_program_id(verifier: &Verifier, program_id: &ProgramId) -> Result<()> {
    match verifier.0.as_str() {
        "risc0" => validate_risc0_program_id(program_id),
        #[cfg(feature = "sp1")]
        "sp1" => hyle_verifiers::validate_sp1_program_id(program_id),
        _ => Ok(()),
    }
}
