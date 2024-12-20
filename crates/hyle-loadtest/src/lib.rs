use std::collections::BTreeMap;

use amm::{AmmState, UnorderedTokenPair};
use anyhow::Error;
use hydentity::Hydentity;
use hyle::{
    model::{BlobTransaction, ContractName, Hashable, ProofData, ProofTransaction},
    rest::client::ApiHttpClient,
    tools::{contract_runner::ContractRunner, transactions_builder::TransactionBuilder},
};
use hyle_contract_sdk::erc20::ERC20;
use hyle_contract_sdk::Digestable;
use hyle_contract_sdk::{flatten_blobs, BlobIndex, HyleOutput};
use hyle_contract_sdk::{identity_provider::IdentityVerification, BlobData};
use hyle_contracts::{AMM_ELF, AMM_ID, HYDENTITY_ELF, HYDENTITY_ID, HYLLAR_ELF, HYLLAR_ID};
use hyllar::{HyllarToken, HyllarTokenContract};

pub fn setup_contract_states(
    number_of_users: &u32,
    pair: &UnorderedTokenPair,
) -> (
    hydentity::Hydentity,
    amm::AmmState,
    hyllar::HyllarToken,
    hyllar::HyllarToken,
) {
    // Setup: create identity contract
    let mut hydentity_state = Hydentity::default();

    // Setup: creating AMM contract
    let mut amm_state = AmmState::default();

    // Setup: creating token1 contract
    let mut balances_token1 = BTreeMap::new();
    let mut allowances_token1 = BTreeMap::new();

    // Setup: creating token2 contract
    let mut balances_token2 = BTreeMap::new();
    let mut allowances_token2 = BTreeMap::new();

    for n in 0..*number_of_users {
        let identity = format!("{}.hydentity-loadtest", n);

        hydentity_state
            .register_identity(&identity, &identity)
            .unwrap();

        balances_token1.insert(identity.clone(), 5);
        allowances_token1.insert(
            (identity.clone(), "amm-loadtest".to_owned()),
            1_000_000_000_000,
        );

        balances_token2.insert(identity.clone(), 5);
        allowances_token2.insert((identity, "amm-loadtest".to_owned()), 1_000_000_000_000);
    }

    // Creation a new pair for these two tokens on the AMM
    balances_token1.insert("amm-loadtest".to_owned(), 1_000_000_000);
    balances_token2.insert("amm-loadtest".to_owned(), 1_000_000_000);
    amm_state.create_new_pair(pair.clone(), (1_000_000_000, 1_000_000_000));

    // Set token1/token2 state with 1_000_000_000_000 tokens, all users with 1000 tokens each and the AMM with allowance to swap for all of them
    let token1_state = HyllarToken::init(1_000_000_000_000, balances_token1, allowances_token1);
    let token2_state = HyllarToken::init(1_000_000_000_000, balances_token2, allowances_token2);

    (hydentity_state, amm_state, token1_state, token2_state)
}

pub async fn register_contracts(
    client: &ApiHttpClient,
    verifier: &str,
    hydentity_state: &Hydentity,
    amm_state: &AmmState,
    token1_state: &HyllarToken,
    token2_state: &HyllarToken,
) -> Result<(), Error> {
    // Create RegisterContract transactions
    let register_hydentity = TransactionBuilder::register_contract(
        "loadtest",
        verifier,
        &HYDENTITY_ID,
        hydentity_state.as_digest(),
        "hydentity-loadtest",
    );
    let register_amm = TransactionBuilder::register_contract(
        "loadtest",
        verifier,
        &AMM_ID,
        amm_state.as_digest(),
        "amm-loadtest",
    );
    let register_token1 = TransactionBuilder::register_contract(
        "loadtest",
        verifier,
        &HYLLAR_ID,
        token1_state.as_digest(),
        "token1-loadtest",
    );
    let register_token2 = TransactionBuilder::register_contract(
        "loadtest",
        verifier,
        &HYLLAR_ID,
        token2_state.as_digest(),
        "token2-loadtest",
    );
    client
        .send_tx_register_contract(&register_hydentity)
        .await?;
    client.send_tx_register_contract(&register_amm).await?;
    client.send_tx_register_contract(&register_token1).await?;
    client.send_tx_register_contract(&register_token2).await?;
    Ok(())
}

pub async fn create_transactions(
    verifier: &str,
    pair: &UnorderedTokenPair,
    hydentity_state: &mut Hydentity,
    amm_state: &mut AmmState,
    token1_state: &mut HyllarToken,
    token2_state: &mut HyllarToken,
    number_of_users: u32,
) -> Result<Vec<(BlobTransaction, Vec<ProofTransaction>)>, Error> {
    // For each user, we create the blob_tx, we compute the transient states of each contracts in order to create the proof_txs
    // We save everything to them next later
    let mut txs_to_send: Vec<(BlobTransaction, Vec<ProofTransaction>)> = vec![];

    for n in 0..number_of_users {
        let identity = format!("{}.hydentity-loadtest", n);

        let mut transaction = TransactionBuilder::new(identity.clone().into());
        transaction
            .verify_identity(
                hydentity_state,
                "hydentity-loadtest".into(),
                identity.clone(),
            )
            .await?;
        transaction.swap(
            identity.clone(),
            "amm-loadtest".into(),
            ("token1-loadtest".to_owned(), "token2-loadtest".to_owned()),
            (5, 5),
        );
        let blob_transaction = transaction.to_blob_transaction();
        let blob_tx_hash = blob_transaction.hash();

        let proofs = generate_proofs(
            verifier,
            pair,
            &blob_transaction,
            hydentity_state,
            amm_state,
            token1_state,
            token2_state,
        )
        .await;

        let proof_tx_hydentity = ProofTransaction {
            blob_tx_hash: blob_tx_hash.clone(),
            proof: proofs.0,
            contract_name: ContractName("hydentity-loadtest".to_owned()),
        };
        let proof_tx_amm = ProofTransaction {
            blob_tx_hash: blob_tx_hash.clone(),
            proof: proofs.1,
            contract_name: ContractName("amm-loadtest".to_owned()),
        };
        let proof_tx_token1 = ProofTransaction {
            blob_tx_hash: blob_tx_hash.clone(),
            proof: proofs.2,
            contract_name: ContractName("token1-loadtest".to_owned()),
        };
        let proof_tx_token2 = ProofTransaction {
            blob_tx_hash: blob_tx_hash.clone(),
            proof: proofs.3,
            contract_name: ContractName("token2-loadtest".to_owned()),
        };

        txs_to_send.push((
            blob_transaction,
            vec![
                proof_tx_hydentity,
                proof_tx_amm,
                proof_tx_token1,
                proof_tx_token2,
            ],
        ));
    }

    Ok(txs_to_send)
}

pub async fn send_transactions(
    client: &ApiHttpClient,
    txs_to_send: Vec<(BlobTransaction, Vec<ProofTransaction>)>,
) -> Result<(), Error> {
    for (blob_tx, _) in txs_to_send.iter() {
        client.send_tx_blob(blob_tx).await?;
    }
    for (_, proof_txs) in txs_to_send.iter() {
        for proof_tx in proof_txs {
            client.send_tx_proof(proof_tx).await?;
        }
    }
    Ok(())
}
async fn generate_proofs(
    verifier: &str,
    pair: &UnorderedTokenPair,
    blob_tx: &BlobTransaction,
    hydentity_state: &mut Hydentity,
    amm_state: &mut AmmState,
    token1_state: &mut HyllarToken,
    token2_state: &mut HyllarToken,
) -> (ProofData, ProofData, ProofData, ProofData) {
    let blob_tx_hash = blob_tx.hash();
    let identity = blob_tx.identity.0.clone();

    match verifier {
        "test" => {
            let initial_state_hydentity = hydentity_state.as_digest();
            let initial_state_amm = amm_state.as_digest();
            let initial_state_token1 = token1_state.as_digest();
            let initial_state_token2 = token2_state.as_digest();

            // Change state of hydentity
            hydentity_state
                .verify_identity(&identity, 0, &identity)
                .unwrap();

            // Change state of amm
            amm_state.update_pair(pair.clone(), (5, 5));

            // Change state of token1
            let mut token1_contract =
                HyllarTokenContract::init(token1_state.clone(), "amm-loadtest".into());
            token1_contract
                .transfer_from(&identity, "amm-loadtest", 5)
                .unwrap();

            // Change state of token2
            let mut token2_contract =
                HyllarTokenContract::init(token2_state.clone(), "amm-loadtest".into());
            token2_contract.transfer(&identity, 5).unwrap();

            let next_state_hydentity = hydentity_state.as_digest();
            let next_state_amm = amm_state.as_digest();
            let next_state_token1 = token1_state.as_digest();
            let next_state_token2 = token2_state.as_digest();

            // Process les hyle_output attendus
            let flatten_blobs = flatten_blobs(&blob_tx.blobs);
            let hyle_output_hydentity = HyleOutput {
                version: 1,
                initial_state: initial_state_hydentity,
                next_state: next_state_hydentity,
                identity: identity.clone().into(),
                tx_hash: blob_tx_hash.clone(),
                index: BlobIndex(0),
                blobs: flatten_blobs.clone(),
                success: true,
                program_outputs: vec![],
            };

            let hyle_output_amm = HyleOutput {
                version: 1,
                initial_state: initial_state_amm,
                next_state: next_state_amm,
                identity: identity.clone().into(),
                tx_hash: blob_tx_hash.clone(),
                index: BlobIndex(1),
                blobs: flatten_blobs.clone(),
                success: true,
                program_outputs: vec![],
            };

            let hyle_output_token1 = HyleOutput {
                version: 1,
                initial_state: initial_state_token1,
                next_state: next_state_token1,
                identity: identity.clone().into(),
                tx_hash: blob_tx_hash.clone(),
                index: BlobIndex(2),
                blobs: flatten_blobs.clone(),
                success: true,
                program_outputs: vec![],
            };

            let hyle_output_token2 = HyleOutput {
                version: 1,
                initial_state: initial_state_token2,
                next_state: next_state_token2,
                identity: identity.clone().into(),
                tx_hash: blob_tx_hash.clone(),
                index: BlobIndex(3),
                blobs: flatten_blobs.clone(),
                success: true,
                program_outputs: vec![],
            };

            let hydentity_proof = ContractRunner::prove_test(hyle_output_hydentity).unwrap();
            let amm_proof = ContractRunner::prove_test(hyle_output_amm).unwrap();
            let token1_proof = ContractRunner::prove_test(hyle_output_token1).unwrap();
            let token2_proof = ContractRunner::prove_test(hyle_output_token2).unwrap();

            (hydentity_proof, amm_proof, token1_proof, token2_proof)
        }
        "risc0" => {
            std::env::set_var("RISC0_DEV_MODE", "1");
            println!("ok");

            let hydentity_runner = ContractRunner::new(
                "hydentity-loadtest".into(),
                HYDENTITY_ELF,
                identity.clone().into(),
                BlobData(identity.clone().into_bytes().to_vec()),
                blob_tx.blobs.clone(),
                BlobIndex(0),
                hydentity_state.clone(),
            )
            .unwrap();
            let amm_runner = ContractRunner::new(
                "amm-loadtest".into(),
                AMM_ELF,
                identity.clone().into(),
                BlobData(vec![]),
                blob_tx.blobs.clone(),
                BlobIndex(1),
                amm_state.clone(),
            )
            .unwrap();
            let token1_runner = ContractRunner::new(
                "token1-loadtest".into(),
                HYLLAR_ELF,
                identity.clone().into(),
                BlobData(vec![]),
                blob_tx.blobs.clone(),
                BlobIndex(2),
                token1_state.clone(),
            )
            .unwrap();
            let token2_runner = ContractRunner::new(
                "token2-loadtest".into(),
                HYLLAR_ELF,
                identity.clone().into(),
                BlobData(vec![]),
                blob_tx.blobs.clone(),
                BlobIndex(3),
                token2_state.clone(),
            )
            .unwrap();

            // Proofs generations
            let hydentity_proof = hydentity_runner.prove().await.unwrap();
            let amm_proof = amm_runner.prove().await.unwrap();
            let token1_proof = token1_runner.prove().await.unwrap();
            let token2_proof = token2_runner.prove().await.unwrap();
            (hydentity_proof, amm_proof, token1_proof, token2_proof)
        }
        _ => todo!(),
    }
}
