use anyhow::{bail, Result};
use hyle_model::{ContractName, ProgramId, StateDigest, Verifier};

use crate::mempool::verifiers::validate_program_id;

/// Check that the new contract name is:
/// - a valid subdomain of the owner contract name.
/// - the exact same domain (for updating the contract).
pub fn validate_contract_name_registration(
    owner: &ContractName,
    new_contract_name: &ContractName,
) -> Result<()> {
    // Special case: 'hyle' TLD is allowed to register new TLD contracts (and can't be updated).
    if owner.0 == "hyle" {
        if new_contract_name.0 != "hyle"
            && !new_contract_name.0.is_empty()
            && !new_contract_name.0.contains(".")
        {
            return Ok(());
        }
    } else if owner == new_contract_name && !owner.0.is_empty() {
        return Ok(());
    }

    let Some((name, tld)) = new_contract_name.0.split_once(".") else {
        bail!(
            "Invalid contract name for '{}': no '.' in {}",
            owner,
            new_contract_name
        );
    };
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
    use hyle_model::StateDigest;

    use crate::model::contract_registration::validate_state_digest_size;

    use super::validate_contract_name_registration;

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
        assert!(validate_contract_name_registration(&owner, &new_contract).is_err());
    }

    #[test]
    fn test_validate_contract_registration_empty_strings() {
        assert!(validate_contract_name_registration(&"".into(), &"".into()).is_err());
        assert!(validate_contract_name_registration(&"".into(), &".".into()).is_err());
        assert!(validate_contract_name_registration(&"a".into(), &"".into()).is_err());
        assert!(validate_contract_name_registration(&"".into(), &"a".into()).is_err());
        assert!(validate_contract_name_registration(&"a".into(), &".".into()).is_err());
        assert!(validate_contract_name_registration(&"".into(), &"a.".into()).is_err());
    }

    #[test]
    fn test_validate_contract_registration_multiple_periods() {
        let owner = "example".into();
        let new_contract = "sub.sub.example".into();
        assert!(validate_contract_name_registration(&owner, &new_contract).is_err());

        let invalid_contract = "sub..example".into();
        assert!(validate_contract_name_registration(&owner, &invalid_contract).is_err());

        let invalid_ending_period = "example.".into();
        assert!(validate_contract_name_registration(&owner, &invalid_ending_period).is_err());
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
}
