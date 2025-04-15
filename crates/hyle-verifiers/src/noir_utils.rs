use anyhow::Error;
use hyle_model::{Blob, BlobIndex, HyleOutput, IndexedBlobs, StateCommitment, TxHash};
use tracing::debug;

/// Extracts the public inputs from the output of `reconstruct_honk_proof`.
pub fn extract_public_inputs(proof_with_public_inputs: &[u8]) -> &[u8] {
    // The first 4 bytes represent the proof size as a big-endian 32-bit unsigned integer.
    let proof_len = u32::from_be_bytes(proof_with_public_inputs[0..4].try_into().unwrap()) as usize;
    // The public inputs are located between the proof size and the proof itself.
    let public_inputs_end = proof_with_public_inputs.len() - proof_len;
    &proof_with_public_inputs[4..public_inputs_end]
}

/// Reverses the flattening process by splitting a `Vec<u8>` into a vector of sanitized hex-encoded strings
pub fn deflatten_fields(flattened_fields: &[u8]) -> Vec<String> {
    const PUBLIC_INPUT_SIZE: usize = 32; // Each field is 32 bytes
    let mut result = Vec::new();

    for chunk in flattened_fields.chunks(PUBLIC_INPUT_SIZE) {
        let hex_string = hex::encode(chunk);
        let sanitised_hex = format!("{:0>64}", hex_string); // Pad to 64 characters
        result.push(sanitised_hex);
    }

    result
}

pub fn parse_noir_output(output: &[u8]) -> Result<HyleOutput, Error> {
    // let mut public_outputs: Vec<String> = serde_json::from_str(&output_json)?;
    let mut vector = deflatten_fields(extract_public_inputs(output));

    let version = u32::from_str_radix(&vector.remove(0), 16)?;
    debug!("Parsed version: {}", version);
    let initial_state = parse_array(&mut vector)?;
    let next_state = parse_array(&mut vector)?;
    let identity = parse_variable_string(&mut vector)?;
    let tx_hash = parse_sized_string(&mut vector, 64)?;
    let index = u32::from_str_radix(&vector.remove(0), 16)?;
    debug!("Parsed index: {}", index);
    let blobs = parse_blobs(&mut vector)?;
    let tx_blob_count = usize::from_str_radix(&vector.remove(0), 16)?;
    debug!("Parsed tx_blob_count: {}", tx_blob_count);
    let success = u32::from_str_radix(&vector.remove(0), 16)? == 1;
    debug!("Parsed success: {}", success);

    Ok(HyleOutput {
        version,
        initial_state: StateCommitment(initial_state),
        next_state: StateCommitment(next_state),
        identity: identity.into(),
        tx_hash: TxHash(tx_hash),
        tx_ctx: None,
        index: BlobIndex(index as usize),
        blobs,
        tx_blob_count,
        success,
        state_reads: vec![],
        onchain_effects: vec![],
        program_outputs: vec![],
    })
}

fn parse_sized_string(vector: &mut Vec<String>, length: usize) -> Result<String, Error> {
    let mut resp = String::with_capacity(length);
    for _ in 0..length {
        let code = u32::from_str_radix(&vector.remove(0), 16)?;
        let ch = std::char::from_u32(code)
            .ok_or_else(|| anyhow::anyhow!("Invalid char code: {}", code))?;
        resp.push(ch);
    }
    debug!("Parsed string: {}", resp);
    Ok(resp)
}

/// Variable string hash trailing zeros
/// total length is always 64 (could be set to height if needed)
/// parsed length is the length of the expected string (without trailing zeros)
fn parse_variable_string(vector: &mut Vec<String>) -> Result<String, Error> {
    let length = usize::from_str_radix(&vector.remove(0), 16)?;
    let mut field = parse_sized_string(vector, 64)?;
    if length > 64 {
        return Err(anyhow::anyhow!(
            "Invalid contract name length {length}. Max is 64."
        ));
    }
    field.truncate(length);
    debug!("Parsed variable string: {}", field);
    Ok(field)
}

fn parse_array(vector: &mut Vec<String>) -> Result<Vec<u8>, Error> {
    let length = usize::from_str_radix(&vector.remove(0), 16)?;
    let mut resp = Vec::with_capacity(length);
    for _ in 0..length {
        let num = u8::from_str_radix(&vector.remove(0), 16)?;
        resp.push(num);
    }
    debug!("Parsed array of len: {}", length);
    Ok(resp)
}

fn parse_blobs(blob_data: &mut Vec<String>) -> Result<IndexedBlobs, Error> {
    let blob_number = usize::from_str_radix(&blob_data.remove(0), 16)?;
    let mut blobs = IndexedBlobs::default();

    debug!("blob_number: {}", blob_number);

    for _ in 0..blob_number {
        let index = usize::from_str_radix(&blob_data.remove(0), 16)?;
        debug!("blob index: {}", index);

        let contract_name = parse_variable_string(blob_data)?;

        let blob_len = usize::from_str_radix(&blob_data.remove(0), 16)?;
        debug!("blob len: {}", blob_len);

        let mut blob = Vec::with_capacity(blob_len);

        for _ in 0..blob_len {
            let v = &blob_data.remove(0);
            blob.push(u8::from_str_radix(v, 16)?);
        }

        debug!("blob data: {:?}", blob);
        blobs.push((
            BlobIndex(index),
            Blob {
                contract_name: contract_name.into(),
                data: hyle_model::BlobData(blob),
            },
        ));
    }

    debug!("Parsed blobs: {:?}", blobs);

    Ok(blobs)
}
