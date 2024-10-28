use assertables::assert_ok;
use std::{fs::File, io::Read};
use testcontainers_modules::{postgres::Postgres, testcontainers::runners::AsyncRunner};

use hyle::{
    indexer::model::ContractDb,
    model::{
        Blob, BlobData, BlobReference, BlobTransaction, ContractName, ProofData, ProofTransaction,
        RegisterContractTransaction,
    },
    node_state::model::Contract,
    rest::client::ApiHttpClient,
};
use hyle_contract_sdk::{Identity, StateDigest, TxHash};
use reqwest::{Client, Url};
use test_helpers::{wait_height, ConfMaker};

mod test_helpers;

use anyhow::{Context, Result};

pub fn load_encoded_receipt_from_file(path: &str) -> Vec<u8> {
    let mut file = File::open(path).expect("Failed to open proof file");
    let mut encoded_receipt = Vec::new();
    file.read_to_end(&mut encoded_receipt)
        .expect("Failed to read file content");
    encoded_receipt
}

async fn register_contracts(client: &ApiHttpClient) -> Result<()> {
    let program_id =
        hex::decode("0f0e89496853ab498a5eda2d06ced45909faf490776c8121063df9066bbb9ea4")
            .expect("Image id decoding failed");
    assert!(client
        .send_tx_register_contract(&RegisterContractTransaction {
            owner: "test".to_string(),
            verifier: "risc0".to_string(),
            program_id,
            state_digest: StateDigest(vec![
                237, 40, 107, 60, 57, 178, 248, 111, 156, 232, 107, 188, 53, 69, 95, 231, 232, 247,
                179, 249, 104, 59, 167, 110, 11, 204, 99, 126, 181, 96, 47, 61
            ]),
            contract_name: ContractName("erc20-risc0".to_string()),
        })
        .await?
        .status()
        .is_success());

    Ok(())
}

async fn send_blobs_and_proofs(client: &ApiHttpClient) -> Result<()> {
    let blob_tx = BlobTransaction {
        identity: Identity("client".to_string()),
        blobs: vec![Blob {
            contract_name: ContractName("erc20-risc0".to_string()),
            data: BlobData(vec![1, 3, 109, 97, 120, 27]),
        }],
    };
    let blob_response = client.send_tx_blob(&blob_tx).await?;

    assert!(blob_response.status().is_success());

    let blob_tx_hash = blob_response.json::<TxHash>().await?;

    let proof = load_encoded_receipt_from_file("./tests/proofs/erc20.risc0.proof");
    let proof_tx = ProofTransaction {
        blobs_references: vec![BlobReference {
            contract_name: ContractName("erc20-risc0".to_string()),
            blob_tx_hash: blob_tx_hash.clone(),
            blob_index: hyle_contract_sdk::BlobIndex(0),
        }],
        proof: ProofData::Bytes(proof),
    };

    assert!(client.send_tx_proof(&proof_tx).await?.status().is_success());

    Ok(())
}

async fn verify_contract_state(client: &ApiHttpClient) -> Result<()> {
    let response = client
        .get_contract(&ContractName("erc20-risc0".to_string()))
        .await?;
    assert!(response.status().is_success(), "{}", response.status());

    let contract = response.json::<Contract>().await?;
    assert_eq!(
        contract.state.0,
        vec![
            154, 65, 139, 95, 54, 114, 201, 168, 66, 153, 34, 153, 43, 237, 17, 198, 0, 39, 64, 81,
            204, 183, 209, 41, 84, 147, 193, 217, 48, 42, 213, 57
        ]
    );

    Ok(())
}

async fn register_test_contracts(client: &ApiHttpClient) -> Result<()> {
    assert_ok!(client
        .send_tx_register_contract(&RegisterContractTransaction {
            owner: "test".to_string(),
            verifier: "test".to_string(),
            program_id: vec![1, 2, 3],
            state_digest: StateDigest(vec![0, 1, 2, 3]),
            contract_name: ContractName("c1".to_string()),
        })
        .await
        .and_then(|response| response.error_for_status().context("registering contract")));

    assert_ok!(client
        .send_tx_register_contract(&RegisterContractTransaction {
            owner: "test".to_string(),
            verifier: "test".to_string(),
            program_id: vec![1, 2, 3],
            state_digest: StateDigest(vec![0, 1, 2, 3]),
            contract_name: ContractName("c2".to_string()),
        })
        .await
        .and_then(|response| response.error_for_status().context("registering contract")));

    Ok(())
}

async fn send_test_blobs_and_proofs(client: &ApiHttpClient) -> Result<()> {
    let blob_response = client
        .send_tx_blob(&BlobTransaction {
            identity: Identity("client".to_string()),
            blobs: vec![
                Blob {
                    contract_name: ContractName("c1".to_string()),
                    data: BlobData(vec![0, 1, 2, 3]),
                },
                Blob {
                    contract_name: ContractName("c2".to_string()),
                    data: BlobData(vec![0, 1, 2, 3]),
                },
            ],
        })
        .await
        .and_then(|response| response.error_for_status().context("sending tx"));

    assert_ok!(blob_response);

    let blob_tx_hash = blob_response.unwrap().json::<TxHash>().await?;

    assert_ok!(client
        .send_tx_proof(&ProofTransaction {
            blobs_references: vec![
                BlobReference {
                    contract_name: ContractName("c1".to_string()),
                    blob_tx_hash: blob_tx_hash.clone(),
                    blob_index: hyle_contract_sdk::BlobIndex(0)
                },
                BlobReference {
                    contract_name: ContractName("c2".to_string()),
                    blob_tx_hash,
                    blob_index: hyle_contract_sdk::BlobIndex(1)
                }
            ],
            proof: ProofData::Bytes(vec![5, 5])
        })
        .await
        .and_then(|response| response.error_for_status().context("sending tx")));

    Ok(())
}

async fn verify_test_contract_state(client: &ApiHttpClient) -> Result<()> {
    let response = client
        .get_contract(&ContractName("c1".to_string()))
        .await
        .and_then(|response| response.error_for_status().context("Getting contract"));
    assert_ok!(response);

    let contract = response.unwrap().json::<Contract>().await?;
    assert_eq!(contract.state.0, vec![4, 5, 6]);

    Ok(())
}

async fn verify_indexer(client: &ApiHttpClient) -> Result<()> {
    let response = client
        .get_indexer_contract(&ContractName("c1".to_string()))
        .await
        .and_then(|response| response.error_for_status().context("Getting contract"));
    assert_ok!(response);

    let contract = response.unwrap().json::<ContractDb>().await?;
    // The indexer returns the initial state
    assert_eq!(contract.state_digest, vec![0, 1, 2, 3]);

    Ok(())
}

#[tokio::test]
async fn e2e() -> Result<()> {
    tracing_subscriber::fmt::init();

    // Start postgres DB with default settings for the indexer.
    let pg = Postgres::default().start().await.unwrap();

    let mut conf_maker = ConfMaker::default();
    conf_maker.default.database_url = format!(
        "postgres://postgres:postgres@localhost:{}/postgres",
        pg.get_host_port_ipv4(5432).await.unwrap()
    );

    // Start 2 nodes
    let node1 = test_helpers::TestProcess::new("node", conf_maker.build())
        .log("hyle=error,tower_http=error")
        .start();

    // Request something on node1 to be sure it's alive and working
    let client_node1 = ApiHttpClient {
        url: Url::parse(&format!("http://{}", &node1.conf.rest)).unwrap(),
        reqwest_client: Client::new(),
    };

    // Wait for node1 to properly spin up
    wait_height(&client_node1, 0).await?;

    let mut node2_conf = conf_maker.build();
    node2_conf.peers = vec![node1.conf.host.clone()];
    let node2 = test_helpers::TestProcess::new("node", node2_conf)
        .log("hyle=error,tower_http=error")
        .start();

    // Wait for node2 to properly spin up
    wait_height(&client_node1, 5).await?;

    // Start a third node just to see what happens.
    let mut node3_conf = conf_maker.build();
    node3_conf.peers = vec![node1.conf.host.clone(), node2.conf.host.clone()];
    let _node3 = test_helpers::TestProcess::new("node", node3_conf).start();

    // Start indexer
    let mut indexer_conf = conf_maker.build();
    indexer_conf.da_address = node2.conf.da_address.clone();
    let indexer = test_helpers::TestProcess::new("indexer", indexer_conf).start();

    // Using a fake proofs
    register_test_contracts(&client_node1).await?;
    send_test_blobs_and_proofs(&client_node1).await?;
    // Using real risc0 proof
    register_contracts(&client_node1).await?;
    send_blobs_and_proofs(&client_node1).await?;

    // Wait for some slots to be finished
    wait_height(&client_node1, 12).await?;

    verify_test_contract_state(&client_node1).await?;
    verify_contract_state(&client_node1).await?;

    // Check that the indexer did index things
    let client_indexer = ApiHttpClient {
        url: Url::parse(&format!("http://{}", &indexer.conf.rest)).unwrap(),
        reqwest_client: Client::new(),
    };

    verify_indexer(&client_indexer).await?;

    //TODO: compare blocks from node1 and node2

    Ok(())
}
