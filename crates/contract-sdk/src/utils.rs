use crate::{
    alloc::string::{String, ToString},
    caller::ExecutionContext,
    guest::fail,
    HyleContract, Identity, StructuredBlobData,
};
use alloc::{collections::BTreeMap, format, vec};
use borsh::{BorshDeserialize, BorshSerialize};
use core::result::Result;

use hyle_model::{
    flatten_blobs, Blob, BlobIndex, ContractInput, DropEndOfReader, HyleOutput, StateCommitment,
    StructuredBlob,
};

/// This function is used to parse the contract input blob data into a given template `Action`
/// It assumes that the blob data is the `Action` serialized with borsh.
/// It returns a tuple with the parsed `Action` and an [ExecutionContext] that can be used
/// by the contract, and will be needed by the sdk to build the [HyleOutput].
///
/// Alternative: [parse_contract_input]
pub fn parse_raw_contract_input<Action>(
    input: &ContractInput,
) -> Result<(Action, ExecutionContext), String>
where
    Action: BorshDeserialize,
{
    let blobs = &input.blobs;
    let index = &input.index;

    let blob = match blobs.get(index) {
        Some(v) => v,
        None => {
            return Err(format!("Could not find Blob at index {index}"));
        }
    };

    let Ok(parameters) = borsh::from_slice::<Action>(blob.data.0.as_slice()) else {
        return Err(format!("Could not deserialize Blob at index {index}"));
    };

    let exec_ctx = ExecutionContext::new(input.identity.clone(), blob.contract_name.clone());
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
/// Alternative: [parse_raw_contract_input]
pub fn parse_contract_input<Action>(
    input: &ContractInput,
) -> Result<(Action, ExecutionContext), String>
where
    Action: BorshSerialize + BorshDeserialize,
{
    let parsed_blob = parse_structured_blob::<Action>(&input.blobs, &input.index);

    let parsed_blob = parsed_blob.ok_or("Failed to parse input blob".to_string())?;

    let caller = check_caller_callees::<Action>(input, &parsed_blob)?;

    let mut callees_blobs = vec::Vec::new();
    for (_, blob) in input.blobs.clone().into_iter() {
        if let Ok(structured_blob) = blob.data.clone().try_into() {
            let structured_blob: StructuredBlobData<DropEndOfReader> = structured_blob; // for type inference
            if structured_blob.caller == Some(input.index) {
                callees_blobs.push(blob);
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
    blobs: &BTreeMap<BlobIndex, Blob>,
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

pub fn as_hyle_output<State: HyleContract + BorshDeserialize>(
    initial_state_commitment: StateCommitment,
    nex_state_commitment: StateCommitment,
    contract_input: ContractInput,
    res: &mut crate::RunResult,
) -> HyleOutput {
    match res {
        Ok((ref mut program_output, execution_context, ref mut onchain_effects)) => {
            if !execution_context.callees_blobs.is_empty() {
                return fail(
                    contract_input,
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
                next_state: nex_state_commitment,
                identity: contract_input.identity,
                index: contract_input.index,
                blobs: flatten_blobs(&contract_input.blobs),
                tx_blob_count: contract_input.tx_blob_count,
                success: true,
                tx_hash: contract_input.tx_hash,
                tx_ctx: contract_input.tx_ctx,
                onchain_effects: core::mem::take(onchain_effects),
                program_outputs: core::mem::take(program_output).into_bytes(),
            }
        }
        Err(message) => fail(contract_input, initial_state_commitment, message),
    }
}

pub fn check_caller_callees<Action>(
    input: &ContractInput,
    parameters: &StructuredBlob<Action>,
) -> Result<Identity, String>
where
    Action: BorshSerialize + BorshDeserialize,
{
    // Check that callees has this blob as caller
    if let Some(callees) = parameters.data.callees.as_ref() {
        for callee_index in callees {
            let callee_blob = input.blobs[callee_index].clone();
            let callee_structured_blob: StructuredBlobData<DropEndOfReader> =
                callee_blob.data.try_into().expect("Failed to decode blob");
            if callee_structured_blob.caller != Some(input.index) {
                return Err("One Callee does not have this blob as caller".to_string());
            }
        }
    }
    // Extract the correct caller
    if let Some(caller_index) = parameters.data.caller.as_ref() {
        let caller_blob = input.blobs[caller_index].clone();
        let caller_structured_blob: StructuredBlobData<DropEndOfReader> =
            caller_blob.data.try_into().expect("Failed to decode blob");
        // Check that caller has this blob as callee
        if caller_structured_blob.callees.is_some()
            && !caller_structured_blob
                .callees
                .unwrap()
                .contains(&input.index)
        {
            return Err("Incorrect Caller for this blob".to_string());
        }
        return Ok(caller_blob.contract_name.0.clone().into());
    }

    // No callers detected, use the identity
    Ok(input.identity.clone())
}
