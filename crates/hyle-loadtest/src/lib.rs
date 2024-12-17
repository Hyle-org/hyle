use std::collections::BTreeMap;

use amm::{AmmState, UnorderedTokenPair};
use anyhow::Error;
use hydentity::Hydentity;
use hyle::{
    model::{BlobTransaction, ContractName, Hashable, ProofTransaction},
    rest::client::ApiHttpClient,
    tools::{contract_runner::ContractRunner, transactions_builder::TransactionBuilder},
};
use hyle_contract_sdk::erc20::ERC20;
use hyle_contract_sdk::identity_provider::IdentityVerification;
use hyle_contract_sdk::Digestable;
use hyle_contract_sdk::{flatten_blobs, BlobIndex, HyleOutput};
use hyle_contracts::{AMM_ID, HYDENTITY_ID, HYLLAR_ID};
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
    hydentity_state: &Hydentity,
    amm_state: &AmmState,
    token1_state: &HyllarToken,
    token2_state: &HyllarToken,
) -> Result<(), Error> {
    // Create RegisterContract transactions
    let register_hydentity = TransactionBuilder::register_contract(
        "loadtest",
        "test",
        &HYDENTITY_ID,
        hydentity_state.as_digest(),
        "hydentity-loadtest",
    );
    let register_amm = TransactionBuilder::register_contract(
        "loadtest",
        "test",
        &AMM_ID,
        amm_state.as_digest(),
        "amm-loadtest",
    );
    let register_token1 = TransactionBuilder::register_contract(
        "loadtest",
        "test",
        &HYLLAR_ID,
        token1_state.as_digest(),
        "token1-loadtest",
    );
    let register_token2 = TransactionBuilder::register_contract(
        "loadtest",
        "test",
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
    pair: &UnorderedTokenPair,
    mut hydentity_state: Hydentity,
    mut amm_state: AmmState,
    token1_state: HyllarToken,
    token2_state: HyllarToken,
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
                &hydentity_state,
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
        let flatten_blobs = flatten_blobs(&blob_transaction.blobs);

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

        let proof_tx_hydentity = ProofTransaction {
            blob_tx_hash: blob_tx_hash.clone(),
            proof: ContractRunner::prove_test(hyle_output_hydentity).unwrap(),
            contract_name: ContractName("hydentity-loadtest".to_owned()),
        };
        let proof_tx_amm = ProofTransaction {
            blob_tx_hash: blob_tx_hash.clone(),
            proof: ContractRunner::prove_test(hyle_output_amm).unwrap(),
            contract_name: ContractName("amm-loadtest".to_owned()),
        };
        let proof_tx_token1 = ProofTransaction {
            blob_tx_hash: blob_tx_hash.clone(),
            proof: ContractRunner::prove_test(hyle_output_token1).unwrap(),
            contract_name: ContractName("token1-loadtest".to_owned()),
        };
        let proof_tx_token2 = ProofTransaction {
            blob_tx_hash: blob_tx_hash.clone(),
            proof: ContractRunner::prove_test(hyle_output_token2).unwrap(),
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
