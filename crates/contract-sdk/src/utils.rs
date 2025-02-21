use crate::{
    alloc::string::{String, ToString},
    caller::ExecutionContext,
    guest::fail,
    Identity, StructuredBlobData,
};
use alloc::{format, vec};
use borsh::{BorshDeserialize, BorshSerialize};
use core::result::Result;

use hyle_model::{
    flatten_blobs, Blob, BlobIndex, ContractInput, Digestable, DropEndOfReader, HyleOutput,
    StateDigest, StructuredBlob,
};

pub fn parse_raw_contract_input<Parameters>(
    input: &ContractInput,
) -> Result<(Parameters, ExecutionContext), String>
where
    Parameters: BorshDeserialize,
{
    let blobs = &input.blobs;
    let index = &input.index;

    let blob = match blobs.get(index.0) {
        Some(v) => v,
        None => {
            return Err(format!("Could not find Blob at index {index}"));
        }
    };

    let Ok(parameters) = borsh::from_slice::<Parameters>(blob.data.0.as_slice()) else {
        return Err(format!("Could not deserialize Blob at index {index}"));
    };

    let exec_ctx = ExecutionContext::default();
    Ok((parameters, exec_ctx))
}

pub fn parse_contract_input<Parameters>(
    input: &ContractInput,
) -> Result<(Parameters, ExecutionContext), String>
where
    Parameters: BorshSerialize + BorshDeserialize,
{
    let parsed_blob = parse_structured_blob::<Parameters>(&input.blobs, &input.index);

    let parsed_blob = parsed_blob.ok_or("Failed to parse input blob".to_string())?;

    let caller = check_caller_callees::<Parameters>(input, &parsed_blob)?;

    let mut callees_blobs = vec::Vec::new();
    for blob in input.blobs.clone().into_iter() {
        if let Ok(structured_blob) = blob.data.clone().try_into() {
            let structured_blob: StructuredBlobData<DropEndOfReader> = structured_blob; // for type inference
            if structured_blob.caller == Some(input.index) {
                callees_blobs.push(blob);
            }
        };
    }

    let ctx = ExecutionContext {
        callees_blobs: callees_blobs.into(),
        caller,
        contract_name: parsed_blob.contract_name.clone(),
    };

    Ok((parsed_blob.data.parameters, ctx))
}

pub fn parse_structured_blob<Parameters>(
    blobs: &[Blob],
    index: &BlobIndex,
) -> Option<StructuredBlob<Parameters>>
where
    Parameters: BorshDeserialize,
{
    let blob = match blobs.get(index.0) {
        Some(v) => v,
        None => {
            return None;
        }
    };

    let parsed_blob: StructuredBlob<Parameters> = match StructuredBlob::try_from(blob.clone()) {
        Ok(v) => v,
        Err(_) => {
            return None;
        }
    };
    Some(parsed_blob)
}

pub fn as_hyle_output<State: Digestable + BorshDeserialize>(
    initial_state_digest: StateDigest,
    nex_state_digest: StateDigest,
    contract_input: ContractInput,
    res: &mut crate::RunResult,
) -> HyleOutput {
    match res {
        Ok((ref mut program_output, execution_context, ref mut registered_contracts)) => {
            if execution_context.callee_blobs().0.is_empty() {
                return fail(
                    contract_input,
                    initial_state_digest,
                    "Execution context has not been fully consumed",
                );
            }
            HyleOutput {
                version: 1,
                initial_state: initial_state_digest,
                next_state: nex_state_digest,
                identity: contract_input.identity,
                index: contract_input.index,
                blobs: flatten_blobs(&contract_input.blobs),
                success: true,
                tx_hash: contract_input.tx_hash,
                tx_ctx: contract_input.tx_ctx,
                registered_contracts: core::mem::take(registered_contracts),
                program_outputs: core::mem::take(program_output).into_bytes(),
            }
        }
        Err(message) => fail(contract_input, initial_state_digest, message),
    }
}

pub fn check_caller_callees<Paramaters>(
    input: &ContractInput,
    parameters: &StructuredBlob<Paramaters>,
) -> Result<Identity, String>
where
    Paramaters: BorshSerialize + BorshDeserialize,
{
    // Check that callees has this blob as caller
    if let Some(callees) = parameters.data.callees.as_ref() {
        for callee_index in callees {
            let callee_blob = input.blobs[callee_index.0].clone();
            let callee_structured_blob: StructuredBlobData<DropEndOfReader> =
                callee_blob.data.try_into().expect("Failed to decode blob");
            if callee_structured_blob.caller != Some(input.index) {
                return Err("One Callee does not have this blob as caller".to_string());
            }
        }
    }
    // Extract the correct caller
    if let Some(caller_index) = parameters.data.caller.as_ref() {
        let caller_blob = input.blobs[caller_index.0].clone();
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
