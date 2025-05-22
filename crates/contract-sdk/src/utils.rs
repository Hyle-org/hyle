use crate::{
    alloc::string::{String, ToString},
    caller::ExecutionContext,
    Identity, StructuredBlobData,
};
use alloc::{format, vec};
use borsh::{BorshDeserialize, BorshSerialize};
use core::result::Result;

use hyle_model::{
    Blob, BlobIndex, Calldata, DropEndOfReader, HyleOutput, IndexedBlobs, StateCommitment,
    StructuredBlob,
};

/// This function is used to parse the contract input blob data into a given template `Action`
/// It assumes that the blob data is the `Action` serialized with borsh.
/// It returns a tuple with the parsed `Action` and an [ExecutionContext] that can be used
/// by the contract, and will be needed by the sdk to build the [HyleOutput].
///
/// Alternative: [parse_calldata]
pub fn parse_raw_calldata<Action>(calldata: &Calldata) -> Result<(Action, ExecutionContext), String>
where
    Action: BorshDeserialize,
{
    let blobs = &calldata.blobs;
    let index = &calldata.index;

    let blob = match blobs.get(index) {
        Some(v) => v,
        None => {
            return Err(format!("Could not find Blob at index {index}"));
        }
    };

    let Ok(parameters) = borsh::from_slice::<Action>(blob.data.0.as_slice()) else {
        return Err(format!("Could not deserialize Blob at index {index}"));
    };

    let exec_ctx = ExecutionContext::new(calldata.identity.clone(), blob.contract_name.clone());
    Ok((parameters, exec_ctx))
}

/// This function is used to parse the contract input blob data.
/// It assumes that the blob data is a [StructuredBlobData] serialized with borsh.
/// It returns a tuple with the parsed `Action` and an [ExecutionContext] that can be used
/// by the contract, and will be needed by the sdk to build the [HyleOutput].
///
/// The [ExecutionContext] will holds the caller/callees information.
/// See [StructuredBlobData] page for more information on caller/callees.
///
/// Alternative: [parse_raw_calldata]
pub fn parse_calldata<Action>(calldata: &Calldata) -> Result<(Action, ExecutionContext), String>
where
    Action: BorshSerialize + BorshDeserialize,
{
    let parsed_blob = parse_structured_blob::<Action>(&calldata.blobs, &calldata.index);

    let parsed_blob = parsed_blob.ok_or("Failed to parse input blob".to_string())?;

    let caller = check_caller_callees::<Action>(calldata, &parsed_blob)?;

    let mut callees_blobs = vec::Vec::new();
    for (_, blob) in &calldata.blobs {
        if let Ok(structured_blob) = blob.data.clone().try_into() {
            let structured_blob: StructuredBlobData<DropEndOfReader> = structured_blob; // for type inference
            if structured_blob.caller == Some(calldata.index) {
                callees_blobs.push(blob.clone());
            }
        };
    }

    let ctx = ExecutionContext {
        callees_blobs,
        caller,
        contract_name: parsed_blob.contract_name.clone(),
    };

    Ok((parsed_blob.data.parameters, ctx))
}

pub fn parse_blob<Action>(blobs: &[Blob], index: &BlobIndex) -> Option<Action>
where
    Action: BorshDeserialize,
{
    let blob = match blobs.get(index.0) {
        Some(v) => v,
        None => {
            return None;
        }
    };

    let Ok(parameters) = borsh::from_slice::<Action>(blob.data.0.as_slice()) else {
        return None;
    };

    Some(parameters)
}

pub fn parse_structured_blob<Action>(
    blobs: &IndexedBlobs,
    index: &BlobIndex,
) -> Option<StructuredBlob<Action>>
where
    Action: BorshDeserialize,
{
    let blob = match blobs.get(index) {
        Some(v) => v,
        None => {
            return None;
        }
    };

    let parsed_blob: StructuredBlob<Action> = match StructuredBlob::try_from(blob.clone()) {
        Ok(v) => v,
        Err(_) => {
            return None;
        }
    };
    Some(parsed_blob)
}

fn fail(
    calldata: &Calldata,
    initial_state_commitment: StateCommitment,
    message: &str,
) -> HyleOutput {
    HyleOutput {
        version: 1,
        initial_state: initial_state_commitment.clone(),
        next_state: initial_state_commitment,
        identity: calldata.identity.clone(),
        index: calldata.index,
        blobs: calldata.blobs.clone(),
        tx_blob_count: calldata.tx_blob_count,
        success: false,
        tx_hash: calldata.tx_hash.clone(),
        state_reads: vec![],
        tx_ctx: calldata.tx_ctx.clone(),
        onchain_effects: vec![],
        program_outputs: message.to_string().into_bytes(),
    }
}

pub fn as_hyle_output(
    initial_state_commitment: StateCommitment,
    next_state_commitment: StateCommitment,
    calldata: &Calldata,
    res: &mut crate::RunResult,
) -> HyleOutput {
    match res {
        Ok((ref mut program_output, execution_context, ref mut onchain_effects)) => {
            if !execution_context.callees_blobs.is_empty() {
                return fail(
                    calldata,
                    initial_state_commitment,
                    &format!(
                        "Execution context has not been fully consumed {:?}",
                        execution_context.callees_blobs
                    ),
                );
            }
            HyleOutput {
                version: 1,
                initial_state: initial_state_commitment,
                next_state: next_state_commitment,
                identity: calldata.identity.clone(),
                index: calldata.index,
                blobs: calldata.blobs.clone(),
                tx_blob_count: calldata.tx_blob_count,
                success: true,
                tx_hash: calldata.tx_hash.clone(),
                state_reads: vec![],
                tx_ctx: calldata.tx_ctx.clone(),
                onchain_effects: core::mem::take(onchain_effects),
                program_outputs: core::mem::take(program_output),
            }
        }
        Err(message) => fail(calldata, initial_state_commitment, message),
    }
}

pub fn check_caller_callees<Action>(
    calldata: &Calldata,
    parameters: &StructuredBlob<Action>,
) -> Result<Identity, String>
where
    Action: BorshSerialize + BorshDeserialize,
{
    // Check that callees has this blob as caller
    if let Some(callees) = parameters.data.callees.as_ref() {
        for callee_index in callees {
            if let Some(callee_blob) = calldata.blobs.get(callee_index).cloned() {
                let callee_structured_blob: StructuredBlobData<DropEndOfReader> =
                    callee_blob.data.try_into().expect("Failed to decode blob");
                if callee_structured_blob.caller != Some(calldata.index) {
                    return Err("One Callee does not have this blob as caller".to_string());
                }
            } else {
                return Err(format!("Callee index {callee_index} not found in blobs"));
            }
        }
    }
    // Extract the correct caller
    if let Some(caller_index) = parameters.data.caller.as_ref() {
        if let Some(caller_blob) = calldata.blobs.get(caller_index).cloned() {
            let caller_structured_blob: StructuredBlobData<DropEndOfReader> =
                caller_blob.data.try_into().expect("Failed to decode blob");
            // Check that caller has this blob as callee
            if caller_structured_blob.callees.is_some()
                && !caller_structured_blob
                    .callees
                    .unwrap()
                    .contains(&calldata.index)
            {
                return Err("Incorrect Caller for this blob".to_string());
            }
            return Ok(caller_blob.contract_name.0.clone().into());
        } else {
            return Err(format!("Caller index {caller_index} not found in blobs"));
        }
    }

    // No callers detected, use the identity
    Ok(calldata.identity.clone())
}
