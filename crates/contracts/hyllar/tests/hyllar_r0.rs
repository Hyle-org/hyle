use core::str;

use hyle_hyllar::{Hyllar, FAUCET_ID};
use sdk::{
    erc20::ERC20Action, BlobIndex, ContractAction, ContractInput, ContractName, HyleOutput, TxHash,
};

fn execute(inputs: ContractInput) -> HyleOutput {
    let contract_input = borsh::to_vec(&inputs).unwrap();
    let env = risc0_zkvm::ExecutorEnv::builder()
        .write(&contract_input.len())
        .unwrap()
        .write_slice(&contract_input)
        .build()
        .unwrap();
    let prover = risc0_zkvm::default_executor();
    let execute_info = prover
        .execute(env, hyle_hyllar::client::metadata::HYLLAR_ELF)
        .unwrap();

    execute_info.journal.decode::<sdk::HyleOutput>().unwrap()
}

#[test]
fn execute_transfer_from() {
    let state = Hyllar::default();
    let output = execute(ContractInput {
        state: borsh::to_vec(&state).unwrap(),
        identity: "caller".into(),
        tx_hash: TxHash::default(),
        tx_ctx: None,
        private_input: vec![],
        blobs: vec![ERC20Action::TransferFrom {
            owner: FAUCET_ID.into(),
            recipient: "amm".into(),
            amount: 100,
        }
        .as_blob(ContractName::new("hyllar"), None, None)],
        index: BlobIndex(0),
    });

    assert!(!output.success);
    assert_eq!(
        str::from_utf8(&output.program_outputs).unwrap(),
        "Allowance exceeded for spender=caller owner=faucet.hydentity allowance=0"
    );
}
