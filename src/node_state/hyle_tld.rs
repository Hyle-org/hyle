use crate::model::contract_registration::validate_contract_registration_metadata;
use crate::model::*;
use anyhow::{bail, Result};
use std::collections::{BTreeMap, HashMap};

use super::SideEffect;

pub fn handle_blob_for_hyle_tld(
    contracts: &HashMap<ContractName, Contract>,
    contract_changes: &mut BTreeMap<ContractName, SideEffect>,
    current_blob: &Blob,
) -> Result<()> {
    // TODO: check the identity of the caller here.

    // TODO: support unstructured blobs as well ?
    if let Ok(reg) =
        StructuredBlobData::<RegisterContractAction>::try_from(current_blob.data.clone())
    {
        handle_register_blob(contracts, contract_changes, &reg.parameters)?;
    } else if let Ok(reg) =
        StructuredBlobData::<DeleteContractAction>::try_from(current_blob.data.clone())
    {
        handle_delete_blob(contracts, contract_changes, &reg.parameters)?;
    } else {
        bail!("Invalid blob data for TLD");
    }
    Ok(())
}

fn handle_register_blob(
    contracts: &HashMap<ContractName, Contract>,
    contract_changes: &mut BTreeMap<ContractName, SideEffect>,
    reg: &RegisterContractAction,
) -> Result<()> {
    // Check name, it's either a direct subdomain or a TLD
    validate_contract_registration_metadata(
        &"hyle".into(),
        &reg.contract_name,
        &reg.verifier,
        &reg.program_id,
        &reg.state_digest,
    )?;

    // Check it's not already registered
    if contracts.contains_key(&reg.contract_name)
        || contract_changes.contains_key(&reg.contract_name)
    {
        bail!("Contract {} is already registered", reg.contract_name.0);
    }

    contract_changes.insert(
        reg.contract_name.clone(),
        SideEffect::Register(Contract {
            name: reg.contract_name.clone(),
            program_id: reg.program_id.clone(),
            state: reg.state_digest.clone(),
            verifier: reg.verifier.clone(),
        }),
    );
    Ok(())
}

fn handle_delete_blob(
    contracts: &HashMap<ContractName, Contract>,
    contract_changes: &mut BTreeMap<ContractName, SideEffect>,
    delete: &DeleteContractAction,
) -> Result<()> {
    // For now, Hylé is allowed to delete all contracts but itself
    if delete.contract_name.0 == "hyle" {
        bail!("Cannot delete Hylé contract");
    }

    // Check it's registered
    if contracts.contains_key(&delete.contract_name)
        || contract_changes.contains_key(&delete.contract_name)
    {
        contract_changes.insert(
            delete.contract_name.clone(),
            SideEffect::Delete(delete.contract_name.clone()),
        );
        Ok(())
    } else {
        bail!("Contract {} is already registered", delete.contract_name.0);
    }
}
