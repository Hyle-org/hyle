use std::collections::{HashMap, VecDeque};

use anyhow::{Context, Error};
use hyle_model::{flatten_blobs, Blob, BlobIndex, HyleOutput, StateCommitment, TxHash};
use tracing::debug;

pub fn parse_noir_output(vector: &mut Vec<String>) -> Result<HyleOutput, Error> {
    let version = u32::from_str_radix(vector.remove(0).strip_prefix("0x").context("parsing")?, 16)?;
    debug!("Parsed version: {}", version);
    let initial_state = parse_array(vector)?;
    let next_state = parse_array(vector)?;
    let identity = parse_variable_string(vector)?;
    let tx_hash = parse_sized_string(vector, 64)?;
    let index = u32::from_str_radix(vector.remove(0).strip_prefix("0x").context("parsing")?, 16)?;
    debug!("Parsed index: {}", index);
    let blobs = parse_blobs(vector)?;
    let tx_blob_count =
        usize::from_str_radix(vector.remove(0).strip_prefix("0x").context("parsing")?, 16)?;
    debug!("Parsed tx_blob_count: {}", tx_blob_count);
    let success =
        u32::from_str_radix(vector.remove(0).strip_prefix("0x").context("parsing")?, 16)? == 1;
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
        onchain_effects: vec![],
        program_outputs: vec![],
    })
}

fn parse_sized_string(vector: &mut Vec<String>, length: usize) -> Result<String, Error> {
    let mut resp = String::with_capacity(length);
    for _ in 0..length {
        let code =
            u32::from_str_radix(vector.remove(0).strip_prefix("0x").context("parsing")?, 16)?;
        let ch = std::char::from_u32(code)
            .ok_or_else(|| anyhow::anyhow!("Invalid char code: {}", code))?;
        resp.push(ch);
    }
    debug!("Parsed string: {}", resp);
    Ok(resp)
}

/// Variable string hash trailing zeros
/// total length is always 64 (could be set to hight if needed)
/// parsed length is the length of the expected string (without trailing zeros)
fn parse_variable_string(vector: &mut Vec<String>) -> Result<String, Error> {
    let length =
        usize::from_str_radix(vector.remove(0).strip_prefix("0x").context("parsing")?, 16)?;
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
    let length =
        usize::from_str_radix(vector.remove(0).strip_prefix("0x").context("parsing")?, 16)?;
    let mut resp = Vec::with_capacity(length);
    for _ in 0..length {
        let num = u8::from_str_radix(vector.remove(0).strip_prefix("0x").context("parsing")?, 16)?;
        resp.push(num);
    }
    debug!("Parsed array of len: {}", length);
    Ok(resp)
}

fn parse_blobs(blob_data: &mut Vec<String>) -> Result<Vec<(BlobIndex, Vec<u8>)>, Error> {
    let blob_number = usize::from_str_radix(
        blob_data.remove(0).strip_prefix("0x").context("parsing")?,
        16,
    )?;
    let mut blobs = HashMap::new();

    debug!("blob_number: {}", blob_number);

    for _ in 0..blob_number {
        let index = usize::from_str_radix(
            blob_data.remove(0).strip_prefix("0x").context("parsing")?,
            16,
        )?;
        debug!("blob index: {}", index);

        let contract_name = parse_variable_string(blob_data)?;

        let blob_len = usize::from_str_radix(
            blob_data.remove(0).strip_prefix("0x").context("parsing")?,
            16,
        )?;
        debug!("blob len: {}", blob_len);

        let mut blob = Vec::with_capacity(blob_len);

        for _ in 0..blob_len {
            let v = &blob_data.remove(0);
            blob.push(u8::from_str_radix(
                v.strip_prefix("0x").context("parsing")?,
                16,
            )?);
        }

        debug!("blob data: {:?}", blob);
        blobs.insert(
            BlobIndex(index),
            Blob {
                contract_name: contract_name.into(),
                data: hyle_model::BlobData(blob),
            },
        );
    }

    let blobs = flatten_blobs(blobs.iter());

    debug!("Parsed blobs: {:?}", blobs);

    Ok(blobs)
}
