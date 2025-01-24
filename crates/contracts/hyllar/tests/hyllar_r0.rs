use core::str;

use hyllar::HyllarToken;
use sdk::{
    erc20::ERC20Action, BlobData, BlobIndex, ContractAction, ContractInput, ContractName,
    Digestable, HyleOutput,
};

fn execute(inputs: ContractInput) -> HyleOutput {
    let env = risc0_zkvm::ExecutorEnv::builder()
        .write(&inputs)
        .unwrap()
        .build()
        .unwrap();
    let prover = risc0_zkvm::default_executor();
    let execute_info = prover
        .execute(env, hyllar::client::metadata::HYLLAR_ELF)
        .unwrap();

    execute_info.journal.decode::<sdk::HyleOutput>().unwrap()
}

#[test]
fn execute_transfer_from() {
    let output = execute(ContractInput {
        initial_state: HyllarToken::new(1000, "faucet".to_string()).as_digest(),
        identity: "caller".into(),
        tx_hash: "".into(),
        private_blob: BlobData(vec![]),
        blobs: vec![ERC20Action::TransferFrom {
            sender: "faucet".into(),
            recipient: "amm".into(),
            amount: 100,
        }
        .as_blob(ContractName::new("hyllar"), None, None)],
        index: BlobIndex(0),
    });

    assert!(!output.success);
    assert_eq!(
        str::from_utf8(&output.program_outputs).unwrap(),
        "Allowance exceeded for sender=faucet caller=caller allowance=0"
    );
}
