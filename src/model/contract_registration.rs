use std::collections::HashSet;

use anyhow::{bail, Result};
use hyle_model::{ContractName, ProgramId, StateDigest, Verifier};

use crate::{mempool::verifiers::validate_program_id, utils::logger::LogMe};

/// Check that the new contract name is:
/// - a valid subdomain of the owner contract name (including sub-subdomains)
/// - the exact same domain (for updating the contract).
pub fn validate_contract_name_registration(
    owner: &ContractName,
    new_contract_name: &ContractName,
) -> Result<()> {
    if owner.0.is_empty() {
        bail!("Invalid contract name for '{}': empty owner", owner);
    }

    // Special case: 'hyle' TLD is allowed to register new TLD contracts (and can't be updated).
    if owner.0 == "hyle" {
        if new_contract_name.0 == "hyle" {
            bail!("Invalid contract name for '{}': can't update 'hyle'", owner);
        } else if !new_contract_name.0.is_empty() && !new_contract_name.0.contains(".") {
            return Ok(());
        }
    // Allow contracts to upgrade themselves
    } else if owner == new_contract_name {
        return Ok(());
    }

    if new_contract_name.0.len() <= owner.0.len() + 1 || !new_contract_name.0.ends_with(&owner.0) {
        bail!(
            "Invalid contract name for '{}': not a subdomain of {}",
            owner,
            new_contract_name
        );
    }

    let Some((name, tld)) = new_contract_name
        .0
        .split_at_checked(new_contract_name.0.len() - owner.0.len() - 1)
    else {
        bail!(
            "Invalid contract name for '{}': no '.' in {}",
            owner,
            new_contract_name
        );
    };
    let tld = &tld[1..];
    if name.is_empty() || tld.is_empty() {
        bail!(
            "Invalid contract name for '{}': empty name/tld in {}",
            owner,
            new_contract_name
        );
    }
    if tld != owner.0 {
        bail!(
            "Invalid subdomain contract name for '{}': {}",
            owner,
            new_contract_name
        );
    }
    if name.split(".").any(|part| part.is_empty()) {
        bail!(
            "Invalid contract name for '{}': empty subdomain in {}",
            owner,
            new_contract_name
        );
    }
    Ok(())
}

/// Check that the state digest is not too long.
pub fn validate_state_digest_size(state_digest: &StateDigest) -> Result<()> {
    const MAX_STATE_DIGEST_SIZE: usize = 10 * 1024 * 1024; // 10 Mb
    if state_digest.0.len() > MAX_STATE_DIGEST_SIZE {
        bail!(
            "StateDigest is too long. Maximum of {MAX_STATE_DIGEST_SIZE} bytes allowed, got {}",
            state_digest.0.len()
        );
    }
    Ok(())
}

pub fn has_item_for_all_parents(
    parents: HashSet<&ContractName>,
    contract_name: &ContractName,
) -> bool {
    let mut subpaths = vec![];
    let parts: Vec<&str> = contract_name.0.split('.').collect();
    for i in 1..parts.len() {
        #[allow(clippy::indexing_slicing, reason = "We are sure that i < parts.len()")]
        subpaths.push(parts[i..].join("."));
    }

    subpaths
        .iter()
        .all(|subpath| parents.contains(&ContractName(subpath.clone())))
}

pub fn validate_contract_registration_metadata(
    owner: &ContractName,
    new_contract_name: &ContractName,
    verifier: &Verifier,
    program_id: &ProgramId,
    state_digest: &StateDigest,
) -> Result<()> {
    validate_contract_name_registration(owner, new_contract_name)?;
    validate_program_id(verifier, program_id)?;
    validate_state_digest_size(state_digest)?;
    Ok(())
}

#[cfg(test)]
mod test {
    use std::collections::HashSet;

    use assertables::assert_ok;
    use hyle_model::{ContractName, StateDigest};

    use crate::model::contract_registration::validate_state_digest_size;

    use super::{has_item_for_all_parents, validate_contract_name_registration};

    #[test]
    fn test_validate_contract_registration_valid_subdomain() {
        let owner = "example".into();
        let new_contract = "sub.example".into();
        assert!(validate_contract_name_registration(&owner, &new_contract).is_ok());
    }

    #[test]
    fn test_validate_contract_registration_invalid_subdomain() {
        let owner = "example".into();
        let new_contract = "another.tld".into();
        assert!(validate_contract_name_registration(&owner, &new_contract).is_err());
    }

    #[test]
    fn test_validate_contract_registration_invalid_format() {
        let owner = "example".into();
        let new_contract = "invalidname".into();
        assert!(validate_contract_name_registration(&owner, &new_contract).is_err());
    }

    #[test]
    fn test_validate_contract_registration_self_registration() {
        let owner = "example".into();
        let new_contract = "example".into();
        assert!(validate_contract_name_registration(&owner, &new_contract).is_ok());
    }

    #[test]
    fn test_validate_contract_registration_hyle_tld() {
        assert!(validate_contract_name_registration(&"hyle".into(), &"newtld".into()).is_ok());
        assert!(validate_contract_name_registration(&"hyle".into(), &"".into()).is_err());
        assert!(validate_contract_name_registration(&"hyle".into(), &".".into()).is_err());
        assert!(validate_contract_name_registration(&"hyle".into(), &"hyle".into()).is_err());
    }

    #[test]
    fn test_validate_contract_registration_hyle_with_subdomains() {
        let owner = "hyle".into();
        let new_contract = "sub.sub.hyle".into();
        assert!(validate_contract_name_registration(&owner, &new_contract).is_ok());
    }

    #[test]
    fn test_validate_contract_registration_empty_strings() {
        assert!(validate_contract_name_registration(&"".into(), &"".into()).is_err());
        assert!(validate_contract_name_registration(&"".into(), &".".into()).is_err());
        assert!(validate_contract_name_registration(&"a".into(), &"".into()).is_err());
        assert!(validate_contract_name_registration(&"".into(), &"a".into()).is_err());
        assert!(validate_contract_name_registration(&"a".into(), &".".into()).is_err());
        assert!(validate_contract_name_registration(&"".into(), &"a.".into()).is_err());
        assert!(validate_contract_name_registration(&"a".into(), &".a".into()).is_err());
        assert!(validate_contract_name_registration(&"".into(), &".a".into()).is_err());
    }

    #[test]
    fn test_validate_contract_registration_multiple_periods() {
        let owner = "example".into();
        let new_contract = "sub.sub.example".into();
        assert!(validate_contract_name_registration(&owner, &new_contract).is_ok());

        let invalid_contract = "sub..example".into();
        assert!(validate_contract_name_registration(&owner, &invalid_contract).is_err());

        let invalid_ending_period = "example.".into();
        assert!(validate_contract_name_registration(&owner, &invalid_ending_period).is_err());
    }

    #[test]
    fn test_validate_contract_registration_multiple_periods_in_owner() {
        let owner = "example.hyle".into();
        let new_contract = "sub.sub.example.hyle".into();
        assert_ok!(validate_contract_name_registration(&owner, &new_contract));
    }

    #[test]
    fn test_validate_contract_registration_case_sensitivity() {
        let owner = "Example".into();
        let new_contract = "sub.example".into();
        assert!(validate_contract_name_registration(&owner, &new_contract).is_err());
    }

    #[test]
    fn test_validate_contract_registration_numeric_names() {
        let owner = "123".into();
        let new_contract = "456.123".into();
        assert!(validate_contract_name_registration(&owner, &new_contract).is_ok());

        let invalid_contract = "123.456".into();
        assert!(validate_contract_name_registration(&owner, &invalid_contract).is_err());
    }

    #[test]
    fn test_validate_contract_registration_smiley() {
        assert!(validate_contract_name_registration(&"hyle".into(), &"🥷".into()).is_ok());
        assert!(
            validate_contract_name_registration(&"hyle".into(), &"💅🏻💅🏼💅🏽💅🏾💅🏿💅".into()).is_ok()
        );
    }

    #[test]
    fn test_validate_state_digest_size_ok() {
        let size = 10 * 1024 * 1024;
        let digest = StateDigest(vec![0; size]);
        assert!(validate_state_digest_size(&digest).is_ok());
    }

    #[test]
    fn test_validate_state_digest_size_too_long() {
        let size = 10 * 1024 * 1024 + 1;
        let digest = StateDigest(vec![0; size]);
        assert!(validate_state_digest_size(&digest).is_err());
    }

    #[test]
    fn test_tx_has_blobs_for_all_parents_all_present() {
        let contract_name = "sub.example.com".into();
        let blob_contract_names = vec!["com".into(), "example.com".into()];
        assert!(has_item_for_all_parents(
            HashSet::from_iter(blob_contract_names.iter()),
            &contract_name
        ));
    }

    #[test]
    fn test_tx_has_blobs_for_all_parents_empty_blobs() {
        let contract_name = "sub.example.com".into();
        let blob_contract_names: Vec<ContractName> = vec![];
        assert!(!has_item_for_all_parents(
            HashSet::from_iter(blob_contract_names.iter()),
            &contract_name
        ));
    }

    #[test]
    fn test_tx_has_blobs_for_all_parents_single_level() {
        let contract_name = "com".into();
        let blob_contract_names = vec!["com".into()];
        assert!(has_item_for_all_parents(
            HashSet::from_iter(blob_contract_names.iter()),
            &contract_name
        ));
    }

    #[test]
    fn test_tx_has_blobs_for_all_parents_multiple_levels() {
        let contract_name = "sub.sub.example.com".into();
        let blob_contract_names =
            vec!["com".into(), "example.com".into(), "sub.example.com".into()];
        assert!(has_item_for_all_parents(
            HashSet::from_iter(blob_contract_names.iter()),
            &contract_name
        ));
    }
}
