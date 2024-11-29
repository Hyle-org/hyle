use std::time::Duration;

use anyhow::{bail, Result};
use bonsai_sdk::non_blocking::Client;
use risc0_zkvm::{compute_image_id, serde::to_vec, Receipt};
use serde::Serialize;
use tracing::info;

pub fn as_input_data<T: Serialize>(data: &T) -> Result<Vec<u8>> {
    let slice = to_vec(data)?;
    Ok(bytemuck::cast_slice(&slice).to_vec())
}

#[allow(dead_code)]
pub async fn run_bonsai(elf: &[u8], input_data: Vec<u8>) -> Result<Receipt> {
    let client = Client::from_env(risc0_zkvm::VERSION)?;

    // Compute the image_id, then upload the ELF with the image_id as its key.
    let image_id = hex::encode(compute_image_id(elf)?);
    client.upload_img(&image_id, elf.to_vec()).await?;

    // Prepare input data and upload it.
    let input_id = client.upload_input(input_data).await?;

    // Add a list of assumptions
    let assumptions: Vec<String> = vec![];

    // Wether to run in execute only mode
    let execute_only = false;

    // Start a session running the prover
    let session = client
        .create_session(image_id, input_id, assumptions, execute_only)
        .await?;
    loop {
        let res = session.status(&client).await?;
        if res.status == "RUNNING" {
            info!(
                "Current status: {} - state: {} - continue polling...",
                res.status,
                res.state.unwrap_or_default()
            );
            std::thread::sleep(Duration::from_secs(15));
            continue;
        }
        if res.status == "SUCCEEDED" {
            // Download the receipt, containing the output
            let receipt_url = res
                .receipt_url
                .expect("API error, missing receipt on completed session");

            let receipt_buf = client.download(&receipt_url).await?;
            let receipt: Receipt = bincode::deserialize(&receipt_buf)?;
            return Ok(receipt);
        } else {
            bail!(
                "Workflow exited: {} - | err: {}",
                res.status,
                res.error_msg.unwrap_or_default()
            );
        }
    }
}
