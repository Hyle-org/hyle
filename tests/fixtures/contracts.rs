use hyle_contract_sdk::Digestable;
use hyle_contract_sdk::ProgramId;
use hyle_contract_sdk::StateDigest;
use hyle_contract_sdk::Verifier;

use super::ctx::E2EContract;

pub struct TestContract {}

impl E2EContract for TestContract {
    fn verifier() -> Verifier {
        "test".into()
    }

    fn program_id() -> ProgramId {
        vec![1, 2, 3].into()
    }

    fn state_digest() -> StateDigest {
        StateDigest(vec![0, 1, 2, 3])
    }
}

pub struct ERC20Contract {}

impl E2EContract for ERC20Contract {
    fn verifier() -> Verifier {
        "risc0".into()
    }

    fn program_id() -> ProgramId {
        hex::decode("0f0e89496853ab498a5eda2d06ced45909faf490776c8121063df9066bbb9ea4")
            .expect("Image id decoding failed")
            .into()
    }

    fn state_digest() -> StateDigest {
        StateDigest(vec![
            237, 40, 107, 60, 57, 178, 248, 111, 156, 232, 107, 188, 53, 69, 95, 231, 232, 247,
            179, 249, 104, 59, 167, 110, 11, 204, 99, 126, 181, 96, 47, 61,
        ])
    }
}

pub struct HydentityContract {}

impl E2EContract for HydentityContract {
    fn verifier() -> Verifier {
        "risc0".into()
    }

    fn program_id() -> ProgramId {
        hyle_contracts::HYDENTITY_ID.to_vec().into()
    }

    fn state_digest() -> StateDigest {
        hydentity::Hydentity::default().as_digest()
    }
}

pub struct HyllarContract {}

impl E2EContract for HyllarContract {
    fn verifier() -> Verifier {
        "risc0".into()
    }

    fn program_id() -> ProgramId {
        hyle_contracts::HYLLAR_ID.to_vec().into()
    }

    fn state_digest() -> StateDigest {
        hyllar::HyllarToken::new(1_000_000_000, "faucet.hydentity".to_string()).as_digest()
    }
}

pub struct AmmContract {}

impl E2EContract for AmmContract {
    fn verifier() -> Verifier {
        "risc0".into()
    }

    fn program_id() -> ProgramId {
        hyle_contracts::AMM_ID.to_vec().into()
    }

    fn state_digest() -> StateDigest {
        amm::AmmState::default().as_digest()
    }
}
