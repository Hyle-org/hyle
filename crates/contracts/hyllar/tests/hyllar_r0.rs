use core::str;

use hyllar::HyllarToken;
use sdk::{
    erc20::ERC20Action, BlobIndex, ContractAction, ContractInput, ContractName, HyleOutput, TxHash,
};

fn execute(inputs: ContractInput<HyllarToken>) -> HyleOutput {
    let inputs = borsh::to_vec(&inputs).unwrap();
    let env = risc0_zkvm::ExecutorEnv::builder()
        .write(&inputs.len())
        .unwrap()
        .write_slice(&inputs)
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
        initial_state: HyllarToken::new(1000, "faucet".to_string()),
        identity: "caller".into(),
        tx_hash: TxHash::default(),
        tx_ctx: None,
        private_input: vec![],
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
