use client_sdk::helpers::risc0::Risc0Prover;
use hyle_smt_token::{
    account::{Account, AccountSMT},
    client::metadata::SMT_TOKEN_ELF,
    utils::BorshableMerkleProof,
    SmtToken, SmtTokenAction,
};
use sdk::{BlobIndex, ContractAction, ContractInput, StateCommitment, TxHash};

#[tokio::main]
async fn main() {
    // Create a new empty SMT
    let mut smt = AccountSMT::default();

    // Create some test accounts
    for user in 0..1000 {
        let account = Account::new(format!("{user}"), 100);
        let key = account.get_key();
        smt.update(key, account).expect("Failed to update SMT");
    }

    let account1 = Account::new("1".to_string(), 100);
    let account2 = Account::new("2".to_string(), 100);

    // Create keys for the accounts
    let key1 = account1.get_key();
    let key2 = account2.get_key();

    // Generate merkle proof for account1
    let proof = smt
        .merkle_proof(vec![key1, key2])
        .expect("Failed to generate proof");

    // Compute initial root
    let root = *smt.root();
    let smt_token = SmtToken::new(StateCommitment(Into::<[u8; 32]>::into(root).to_vec()));

    let token_action = SmtTokenAction::Transfer {
        proof: BorshableMerkleProof(proof),
        sender_account: account1,
        recipient_account: account2,
        amount: 100,
    };

    let contract_input = ContractInput {
        state: borsh::to_vec(&smt_token.commitment.0).unwrap(),
        index: BlobIndex(0),
        identity: "alice".into(),
        blobs: vec![token_action.as_blob("verkle-token".into(), None, None)],
        tx_hash: TxHash::default(),
        tx_ctx: None,
        private_input: vec![],
    };

    let prover = Risc0Prover::new(SMT_TOKEN_ELF);

    let proof = prover.prove(contract_input).await;

    if let Err(err) = proof {
        println!("Error: {:?}", err);
        return;
    }
    println!("proof size: {:?}", proof.unwrap().0.len());
    return;
}
