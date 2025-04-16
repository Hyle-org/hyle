use core::str;

use hyle_hyllar::{Hyllar, HyllarAction, FAUCET_ID};
use sdk::{BlobIndex, Calldata, ContractAction, ContractName, HyleOutput, TxHash};

fn execute(inputs: (Vec<u8>, Calldata)) -> HyleOutput {
    let inputs = borsh::to_vec(&inputs).unwrap();
    let env = risc0_zkvm::ExecutorEnv::builder()
        .write(&inputs.len())
        .unwrap()
        .write_slice(&inputs)
        .build()
        .unwrap();
    let prover = risc0_zkvm::default_executor();
    let execute_info = prover
        .execute(
            env,
            hyle_hyllar::client::tx_executor_handler::metadata::HYLLAR_ELF,
        )
        .unwrap();

    execute_info.journal.decode::<sdk::HyleOutput>().unwrap()
}

#[test]
fn execute_transfer_from() {
    let state = Hyllar::default();
    let output = execute((
        borsh::to_vec(&state).unwrap(),
        Calldata {
            identity: "caller".into(),
            tx_hash: TxHash::default(),
            tx_ctx: None,
            private_input: vec![],
            blobs: vec![HyllarAction::TransferFrom {
                owner: FAUCET_ID.into(),
                recipient: "amm".into(),
                amount: 100,
            }
            .as_blob(ContractName::new("hyllar"), None, None)]
            .into(),
            tx_blob_count: 1,
            index: BlobIndex(0),
        },
    ));

    assert!(!output.success);
    assert_eq!(
        str::from_utf8(&output.program_outputs).unwrap(),
        "Allowance exceeded for spender=caller owner=faucet@hydentity allowance=0"
    );
}
