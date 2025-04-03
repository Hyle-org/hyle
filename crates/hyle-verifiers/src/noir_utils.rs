use std::collections::VecDeque;

use anyhow::{Context, Error};
use hyle_model::{BlobIndex, HyleOutput, StateCommitment, TxHash};

pub fn parse_noir_output(vector: &mut Vec<String>) -> Result<HyleOutput, Error> {
    let version = u32::from_str_radix(vector.remove(0).strip_prefix("0x").context("parsing")?, 16)?;
    let initial_state = parse_array(vector)?;
    let next_state = parse_array(vector)?;
    let identity = parse_string(vector)?;
    let tx_hash = parse_string(vector)?;
    let index = u32::from_str_radix(vector.remove(0).strip_prefix("0x").context("parsing")?, 16)?;
    let blobs = parse_blobs(vector)?;
    let tx_blob_count =
        usize::from_str_radix(vector.remove(0).strip_prefix("0x").context("parsing")?, 16)?;
    let success =
        u32::from_str_radix(vector.remove(0).strip_prefix("0x").context("parsing")?, 16)? == 1;

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

fn parse_string(vector: &mut Vec<String>) -> Result<String, Error> {
    let length =
        usize::from_str_radix(vector.remove(0).strip_prefix("0x").context("parsing")?, 16)?;
    let mut resp = String::with_capacity(length);
    for _ in 0..length {
        let code =
            u32::from_str_radix(vector.remove(0).strip_prefix("0x").context("parsing")?, 16)?;
        let ch = std::char::from_u32(code)
            .ok_or_else(|| anyhow::anyhow!("Invalid char code: {}", code))?;
        resp.push(ch);
    }
    Ok(resp)
}

fn parse_array(vector: &mut Vec<String>) -> Result<Vec<u8>, Error> {
    let length =
        usize::from_str_radix(vector.remove(0).strip_prefix("0x").context("parsing")?, 16)?;
    let mut resp = Vec::with_capacity(length);
    for _ in 0..length {
        let num = u8::from_str_radix(vector.remove(0).strip_prefix("0x").context("parsing")?, 16)?;
        resp.push(num);
    }
    Ok(resp)
}

fn parse_blobs(vector: &mut Vec<String>) -> Result<Vec<(BlobIndex, Vec<u8>)>, Error> {
    let mut blob_data = VecDeque::from(vector.clone());

    let blob_number = usize::from_str_radix(
        blob_data
            .pop_front()
            .ok_or_else(|| anyhow::anyhow!("Missing blob number"))?
            .strip_prefix("0x")
            .context("parsing")?,
        16,
    )?;
    let mut blobs = Vec::new();

    for _ in 0..blob_number {
        let index =
            u32::from_str_radix(vector.remove(0).strip_prefix("0x").context("parsing")?, 16)?;

        let blob_size = usize::from_str_radix(
            blob_data
                .pop_front()
                .ok_or_else(|| anyhow::anyhow!("Missing blob size"))?
                .strip_prefix("0x")
                .context("parsing")?,
            16,
        )?;

        let mut blob = Vec::with_capacity(blob_size);

        for _ in 0..blob_size {
            let v = &blob_data
                .pop_front()
                .ok_or_else(|| anyhow::anyhow!("Missing blob data"))?;
            blob.push(u8::from_str_radix(
                v.strip_prefix("0x").context("parsing")?,
                16,
            )?);
        }
        blobs.push((BlobIndex(index as usize), blob));
    }

    Ok(blobs)
}
