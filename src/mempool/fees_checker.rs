use anyhow::{bail, Result};
use hyle_contract_sdk::Identity;

use crate::model::*;

pub fn check_fees(tx: &Transaction) -> Result<()> {
    match &tx.transaction_data {
        TransactionData::Blob(blob_tx) => check_blob_fees(blob_tx),
        _ => Ok(()),
    }
}

fn check_blob_fees(tx: &BlobTransaction) -> Result<()> {
    let fee_id = check_contract(&tx.fees.fee)?;
    let id = check_contract(&tx.fees.identity)?;
    if fee_id != id {
        bail!("Fee payer identity does not match identity in blob");
    }
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
