use hyle_contract_sdk::Digestable;
use hyle_contract_sdk::ProgramId;
use hyle_contract_sdk::StateDigest;
use hyle_contract_sdk::Verifier;
use hyllar::Hyllar;

use super::ctx::E2EContract;

pub struct ERC20TestContract {}

impl E2EContract for ERC20TestContract {
    fn verifier() -> Verifier {
        hyle_verifiers::versions::RISC0_1.into()
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

pub struct HydentityTestContract {}

impl E2EContract for HydentityTestContract {
    fn verifier() -> Verifier {
        hyle_verifiers::versions::RISC0_1.into()
    }

    fn program_id() -> ProgramId {
        hyle_contracts::HYDENTITY_ID.to_vec().into()
    }

    fn state_digest() -> StateDigest {
        hydentity::Hydentity::default().as_digest()
    }
}

pub struct HyllarTestContract {}

impl HyllarTestContract {
    pub fn init_state() -> Hyllar {
        hyllar::Hyllar::default()
    }
}

impl E2EContract for HyllarTestContract {
    fn verifier() -> Verifier {
        hyle_verifiers::versions::RISC0_1.into()
    }

    fn program_id() -> ProgramId {
        hyle_contracts::HYLLAR_ID.to_vec().into()
    }

    fn state_digest() -> StateDigest {
        HyllarTestContract::init_state().as_digest()
    }
}

pub struct AmmTestContract {}

impl E2EContract for AmmTestContract {
    fn verifier() -> Verifier {
        hyle_verifiers::versions::RISC0_1.into()
    }

    fn program_id() -> ProgramId {
        hyle_contracts::AMM_ID.to_vec().into()
    }

    fn state_digest() -> StateDigest {
        amm::Amm::default().as_digest()
    }
}
