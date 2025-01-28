use crate::{
    alloc::string::{String, ToString},
    alloc::vec::Vec,
    Identity, StructuredBlobData,
};
use bincode::{Decode, Encode};
use core::result::Result;

use hyle_model::{
    flatten_blobs, Blob, BlobIndex, ContractInput, Digestable, HyleOutput, StructuredBlob,
};

pub fn parse_blob<Parameters>(blobs: &[Blob], index: &BlobIndex) -> Option<Parameters>
where
    Parameters: Decode,
{
    let blob = match blobs.get(index.0) {
        Some(v) => v,
        None => {
            return None;
        }
    };

    let Ok((parameters, _)) = bincode::decode_from_slice::<Parameters, _>(
        blob.data.0.as_slice(),
        bincode::config::standard(),
    ) else {
        return None;
    };

    Some(parameters)
}

pub fn parse_structured_blob<Parameters>(
    blobs: &[Blob],
    index: &BlobIndex,
) -> Option<StructuredBlob<Parameters>>
where
    Parameters: Decode,
{
    let blob = match blobs.get(index.0) {
        Some(v) => v,
        None => {
            return None;
        }
    };

    let parsed_blob: StructuredBlob<Parameters> = StructuredBlob::try_from(blob.clone())
        .unwrap_or_else(|e| {
            panic!("Failed to decode blob: {:?}", e);
        });
    Some(parsed_blob)
}

pub fn as_hyle_output<State>(
    input: ContractInput,
    new_state: State,
    res: crate::RunResult,
) -> HyleOutput
where
    State: Digestable,
{
    HyleOutput {
        version: 1,
        initial_state: input.initial_state,
        next_state: new_state.as_digest(),
        identity: input.identity,
        tx_hash: input.tx_hash,
        index: input.index,
        blobs: flatten_blobs(&input.blobs),
        success: res.is_ok(),
        program_outputs: res.unwrap_or_else(|e| e.to_string()).into_bytes(),
    }
}

pub fn check_caller_callees<Paramaters>(
    input: &ContractInput,
    parameters: &StructuredBlob<Paramaters>,
) -> Result<Identity, String>
where
    Paramaters: Encode + Decode,
{
    // Check that callees has this blob as caller
    if let Some(callees) = parameters.data.callees.as_ref() {
        for callee_index in callees {
            let callee_blob = input.blobs[callee_index.0].clone();
            let callee_structured_blob: StructuredBlobData<Vec<u8>> =
                callee_blob.data.try_into().expect("Failed to decode blob");
            if callee_structured_blob.caller != Some(input.index) {
                return Err("One Callee does not have this blob as caller".to_string());
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
            return Err("Incorrect Caller for this blob".to_string());
        }
        return Ok(caller_blob.contract_name.0.clone().into());
    }

    // No callers detected, use the identity
    Ok(input.identity.clone())
}
