use anyhow::{bail, Result};
use hyle_contract_sdk::Identity;
use tracing::info;

use crate::model::*;

pub fn check_fees(tx: &Transaction) -> Result<()> {
    match &tx.transaction_data {
        TransactionData::Blob(blob_tx) => check_blob_fees(blob_tx),
        _ => Ok(()),
    }
}

fn check_blob_fees(tx: &BlobTransaction) -> Result<()> {
    let ids = tx
        .fees
        .blobs
        .iter()
        .map(check_contract)
        .collect::<Result<Vec<_>>>()?;

    if !ids.windows(2).all(|w| w[0] == w[1]) {
        bail!("Fees blob identities does not match");
    }
    if tx.fees.identity != ids[0] {
        bail!("Fees identity does not match");
    }

    info!("💸 Fees will be paid by: {}", ids[0].0);
    Ok(())
}

fn check_contract(blob: &Blob) -> Result<Identity> {
    match blob.contract_name.0.as_str() {
        "hyfi" => check_hyfi_contract(blob),
        "hydentity" => check_hydentity_contract(blob),
        _ => bail!("Invalid contract name: {}", blob.contract_name.0),
    }
}

fn check_hyfi_contract(blob: &Blob) -> Result<Identity> {
    let parameters = hyfi::model::ContractFunction::decode(&blob.data)?;
    let mut state = hyfi::model::Balances::default(); // TODO: fetch state from chain
    let result = hyfi::run(&mut state, parameters);
    if !result.success {
        bail!("Failed to pay fees");
    }
    Ok(result.identity.into())
}

fn check_hydentity_contract(blob: &Blob) -> Result<Identity> {
    let parameters = hydentity::model::ContractFunction::decode(&blob.data)?;
    let mut state = hydentity::model::Identities::default(); // TODO: fetch state from chain
    let result = hydentity::run(&mut state, parameters);
    if !result.success {
        bail!("Failed to verify identity");
    }
    Ok(result.identity.into())
}
