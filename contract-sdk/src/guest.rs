use alloc::{string::ToString, vec::Vec};
use anyhow::{bail, Result};
use bincode::{Decode, Encode};
use serde::de::DeserializeOwned;

use crate::{
    flatten_blobs,
    utils::{parse_blob, parse_structured_blob},
    ContractInput, Digestable, HyleOutput, Identity, StructuredBlob, StructuredBlobData,
};

#[cfg(feature = "risc0")]
pub mod env {
    use super::*;

    pub fn log(message: &str) {
        risc0_zkvm::guest::env::log(message);
    }

    pub fn commit(output: &HyleOutput) {
        risc0_zkvm::guest::env::commit(output);
    }

    pub fn read<T: DeserializeOwned>() -> T {
        risc0_zkvm::guest::env::read()
    }
}
// For coverage tests, assume risc0 if both are active
#[cfg(all(feature = "sp1", not(feature = "risc0")))]
pub mod env {
    use super::*;

    pub fn log(message: &str) {
        // TODO: this does nothing actually
        sp1_zkvm::io::hint(&message);
    }

    pub fn commit(output: &HyleOutput) {
        sp1_zkvm::io::commit(output);
    }

    pub fn read<T: DeserializeOwned>() -> T {
        sp1_zkvm::io::read()
    }
}

pub fn fail<State>(input: ContractInput<State>, message: &str)
where
    State: Digestable,
{
    env::log(message);

    env::commit(&HyleOutput {
        version: 1,
        initial_state: input.initial_state.as_digest(),
        next_state: input.initial_state.as_digest(),
        identity: input.identity,
        tx_hash: input.tx_hash,
        index: input.index,
        blobs: flatten_blobs(&input.blobs),
        success: false,
        program_outputs: message.to_string().into_bytes(),
    });
}

pub fn panic(message: &str) {
    env::log(message);
    // should we env::commit ?
    panic!("{}", message);
}

pub fn init_raw<State, Parameters>() -> (ContractInput<State>, Parameters)
where
    State: Digestable + DeserializeOwned,
    Parameters: Decode,
{
    let input: ContractInput<State> = env::read();

    let parsed_blob = parse_blob::<Parameters>(&input.blobs, &input.index);

    (input, parsed_blob)
}

pub fn init_with_caller<State, Parameters>(
) -> Result<(ContractInput<State>, StructuredBlob<Parameters>, Identity)>
where
    State: Digestable + DeserializeOwned,
    Parameters: Encode + Decode,
{
    let input: ContractInput<State> = env::read();

    let parsed_blob = parse_structured_blob::<Parameters>(&input.blobs, &input.index);

    let caller = check_caller_callees::<State, Parameters>(&input, &parsed_blob)?;

    Ok((input, parsed_blob, caller))
}

pub fn commit<State>(input: ContractInput<State>, new_state: State, res: crate::RunResult)
where
    State: Digestable,
{
    env::commit(&HyleOutput {
        version: 1,
        initial_state: input.initial_state.as_digest(),
        next_state: new_state.as_digest(),
        identity: input.identity,
        tx_hash: input.tx_hash,
        index: input.index,
        blobs: flatten_blobs(&input.blobs),
        success: res.is_ok(),
        program_outputs: res.unwrap_or_else(|e| e.to_string()).into_bytes(),
    });
}

pub fn check_caller_callees<State, Paramaters>(
    input: &ContractInput<State>,
    parameters: &StructuredBlob<Paramaters>,
) -> Result<Identity>
where
    State: Digestable,
    Paramaters: Encode + Decode,
{
    // Check that callees has this blob as caller
    if let Some(callees) = parameters.data.callees.as_ref() {
        for callee_index in callees {
            let callee_blob = input.blobs[callee_index.0].clone();
            let callee_structured_blob: StructuredBlobData<Vec<u8>> =
                callee_blob.data.try_into().expect("Failed to decode blob");
            if callee_structured_blob.caller != Some(input.index.clone()) {
                bail!("One Callee does not have this blob as caller");
            }
        }
    }
    // Extract the correct caller
    if let Some(caller_index) = parameters.data.caller.as_ref() {
        let caller_blob = input.blobs[caller_index.0].clone();
        let caller_structured_blob: StructuredBlobData<Vec<u8>> =
            caller_blob.data.try_into().expect("Failed to decode blob");
        // Check that caller has this blob as callee
        if caller_structured_blob.callees.is_some()
            && !caller_structured_blob
                .callees
                .unwrap()
                .contains(&input.index)
        {
            bail!("Incorrect Caller for this blob");
        }
        return Ok(caller_blob.contract_name.0.clone().into());
    }

    // No callers detected, use the identity
    Ok(input.identity.clone())
}
