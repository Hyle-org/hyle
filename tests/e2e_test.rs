use hyle::{
    consensus::{Consensus, ConsensusStore},
    indexer::model::ContractDb,
    model::{
        Blob, BlobData, BlobReference, BlobTransaction, ContractName, Identity, ProofTransaction,
        RegisterContractTransaction, StateDigest, TxHash,
    },
    node_state::model::Contract,
    rest::client::ApiHttpClient,
    utils::modules::Module,
};
use reqwest::{Client, Url};
use std::{
    fs::{self, File},
    io::Read,
    path::Path,
    time,
};
use test_helpers::NodeType;
use tokio::time::sleep;

mod test_helpers;

use anyhow::Result;

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
            blob_index: hyle::model::BlobIndex(0),
        }],
        proof,
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
    assert!(client
        .send_tx_register_contract(&RegisterContractTransaction {
            owner: "test".to_string(),
            verifier: "test".to_string(),
            program_id: vec![1, 2, 3],
            state_digest: StateDigest(vec![0, 1, 2, 3]),
            contract_name: ContractName("c1".to_string()),
        })
        .await?
        .status()
        .is_success());

    assert!(client
        .send_tx_register_contract(&RegisterContractTransaction {
            owner: "test".to_string(),
            verifier: "test".to_string(),
            program_id: vec![1, 2, 3],
            state_digest: StateDigest(vec![0, 1, 2, 3]),
            contract_name: ContractName("c2".to_string()),
        })
        .await?
        .status()
        .is_success());

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
        .await?;

    assert!(blob_response.status().is_success());

    let blob_tx_hash = blob_response.json::<TxHash>().await?;

    assert!(client
        .send_tx_proof(&ProofTransaction {
            blobs_references: vec![
                BlobReference {
                    contract_name: ContractName("c1".to_string()),
                    blob_tx_hash: blob_tx_hash.clone(),
                    blob_index: hyle::model::BlobIndex(0)
                },
                BlobReference {
                    contract_name: ContractName("c2".to_string()),
                    blob_tx_hash,
                    blob_index: hyle::model::BlobIndex(1)
                }
            ],
            proof: vec![5, 5]
        })
        .await?
        .status()
        .is_success());

    Ok(())
}

async fn verify_test_contract_state(client: &ApiHttpClient) -> Result<()> {
    let response = client.get_contract(&ContractName("c1".to_string())).await?;
    assert!(response.status().is_success(), "{}", response.status());

    let contract = response.json::<Contract>().await?;
    assert_eq!(contract.state.0, vec![4, 5, 6]);

    Ok(())
}

async fn verify_indexer(client: &ApiHttpClient) -> Result<()> {
    let response = client
        .get_indexer_contract(&ContractName("c1".to_string()))
        .await?;
    assert!(response.status().is_success(), "{}", response.status());

    let contract = response.json::<ContractDb>().await?;
    // The indexer returns the initial state
    assert_eq!(contract.state_digest, vec![0, 1, 2, 3]);

    Ok(())
}

#[tokio::test]
async fn e2e() -> Result<()> {
    tracing_subscriber::fmt::init();

    // FIXME: use tmp dir
    let path_node1 = Path::new("tests/node1");
    let path_node2 = Path::new("tests/node2");

    // Clean created files
    _ = fs::remove_dir_all(path_node1.join("data_node1"));
    _ = fs::remove_dir_all(path_node2.join("data_node2"));

    // Start 2 nodes
    let node1 = test_helpers::TestNode::new(path_node1, NodeType::Node, "6668");
    let node2 = test_helpers::TestNode::new(path_node2, NodeType::Node, "6669");

    // Wait for node to properly spin up
    sleep(time::Duration::from_secs(2)).await;

    // Start indexer
    let indexer = test_helpers::TestNode::new(path_node1, NodeType::Indexer, "6670");

    // Request something on node1 to be sure it's alive and working
    let client_node1 = ApiHttpClient {
        url: Url::parse("http://localhost:4321").unwrap(),
        reqwest_client: Client::new(),
    };

    // Using a fake proofs
    register_test_contracts(&client_node1).await?;
    send_test_blobs_and_proofs(&client_node1).await?;
    // Using real risc0 proof
    register_contracts(&client_node1).await?;
    send_blobs_and_proofs(&client_node1).await?;

    // Wait for some slots to be finished
    sleep(time::Duration::from_secs(15)).await;

    verify_test_contract_state(&client_node1).await?;
    verify_contract_state(&client_node1).await?;

    // Check that the indexer did index things
    let client_indexer = ApiHttpClient {
        url: Url::parse("http://localhost:5544").unwrap(),
        reqwest_client: Client::new(),
    };

    verify_indexer(&client_indexer).await?;

    // Stop all processes
    drop(indexer);
    drop(node1);
    drop(node2);

    // Check that some blocks has been produced
    let node1_consensus: ConsensusStore =
        Consensus::load_from_disk_or_default(path_node1.join("data_node1/consensus.bin").as_path());
    let node2_consensus: ConsensusStore =
        Consensus::load_from_disk_or_default(path_node2.join("data_node2/consensus.bin").as_path());
    assert!(!node1_consensus.blocks.is_empty());
    assert!(!node2_consensus.blocks.is_empty());
    // FIXME: check that created blocks are the same.

    // Clean created files
    fs::remove_dir_all(path_node1.join("data_node1")).expect("file cleaning failed");
    fs::remove_dir_all(path_node2.join("data_node2")).expect("file cleaning failed");

    //TODO: compare blocks from node1 and node2

    Ok(())
}
