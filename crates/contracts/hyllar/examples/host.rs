use client_sdk::helpers::risc0::Risc0Prover;
use hyle_hyllar::erc20::ERC20;
use hyle_hyllar::{client::metadata::HYLLAR_ELF, Hyllar, HyllarAction, FAUCET_ID};
use sdk::{BlobIndex, ContractAction, ContractInput, TxHash};

#[tokio::main]
async fn main() {
    let mut hyllar = Hyllar::default();
    let users = 1000;
    for n in 0..users {
        let ident = &format!("{n}");
        hyllar
            .transfer(FAUCET_ID, ident, 0)
            .map_err(|e| anyhow::anyhow!(e))
            .unwrap();
    }

    let hyllar_action = HyllarAction::Transfer {
        recipient: "alice".to_string(),
        amount: 0,
    };

    let contract_input = ContractInput {
        state: hyllar.to_bytes(),
        index: BlobIndex(0),
        identity: FAUCET_ID.into(),
        blobs: vec![hyllar_action.as_blob("hyllar".into(), None, None)],
        tx_hash: TxHash::default(),
        tx_ctx: None,
        private_input: vec![],
    };

    let prover = Risc0Prover::new(HYLLAR_ELF);
    let proof = prover.prove(contract_input).await;

    if let Err(err) = proof {
        println!("Error: {:?}", err);
        return;
    }
    println!("proof size: {:?}", proof.unwrap().0.len());
    return;
}
