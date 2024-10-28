use hyle_contract_sdk::StateDigest;

use super::ctx::E2EContract;

pub struct TestContract {}

impl E2EContract for TestContract {
    fn verifier() -> String {
        "test".into()
    }

    fn program_id() -> Vec<u8> {
        vec![1, 2, 3]
    }

    fn state_digest() -> StateDigest {
        StateDigest(vec![0, 1, 2, 3])
    }
}

pub struct ERC20Contract {}

impl E2EContract for ERC20Contract {
    fn verifier() -> String {
        "risc0".into()
    }

    fn program_id() -> Vec<u8> {
        hex::decode("0f0e89496853ab498a5eda2d06ced45909faf490776c8121063df9066bbb9ea4")
            .expect("Image id decoding failed")
    }

    fn state_digest() -> StateDigest {
        StateDigest(vec![
            237, 40, 107, 60, 57, 178, 248, 111, 156, 232, 107, 188, 53, 69, 95, 231, 232, 247,
            179, 249, 104, 59, 167, 110, 11, 204, 99, 126, 181, 96, 47, 61,
        ])
    }
}
