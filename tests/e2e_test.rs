use hyle::{
    consensus::Consensus,
    model::{
        Blob, BlobData, BlobReference, BlobTransaction, ContractName, Identity, ProofTransaction,
        RegisterContractTransaction, StateDigest, TxHash,
    },
    node_state::model::Contract,
};
use reqwest::blocking::{Client, Response};
use serde::Serialize;
use std::{fs, path::Path, thread, time};

mod test_helpers;

fn send<T: Serialize>(client: &Client, url: String, obj: T) -> Response {
    let request_body = serde_json::json!(obj);

    client
        .post(url)
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .expect("Failed to send request")
}

fn url(path: &str) -> String {
    format!("http://127.0.0.1:4321{}", path)
}

fn register_contracts(client: &Client) {
    assert!(send(
        client,
        url("/v1/contract/register"),
        RegisterContractTransaction {
            owner: "test".to_string(),
            verifier: "test".to_string(),
            program_id: vec![1, 2, 3],
            state_digest: StateDigest(vec![0, 1, 2, 3]),
            contract_name: ContractName("c1".to_string()),
        },
    )
    .status()
    .is_success());

    assert!(send(
        client,
        url("/v1/contract/register"),
        RegisterContractTransaction {
            owner: "test".to_string(),
            verifier: "test".to_string(),
            program_id: vec![1, 2, 3],
            state_digest: StateDigest(vec![0, 1, 2, 3]),
            contract_name: ContractName("c2".to_string()),
        },
    )
    .status()
    .is_success());
}

fn send_blobs(client: &Client) {
    let blob_response = send(
        client,
        url("/v1/tx/send/blob"),
        BlobTransaction {
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
        },
    );

    assert!(blob_response.status().is_success());
    let blob_tx_hash = blob_response
        .json::<TxHash>()
        .expect("Failed to parse tx hash");

    assert!(send(
        client,
        url("/v1/tx/send/proof"),
        ProofTransaction {
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
        },
    )
    .status()
    .is_success());
}

fn verify_contract_state(client: &Client) {
    let response = client
        .get(url("/v1/contract/c1"))
        .header("Content-Type", "application/json")
        .send()
        .expect("Failed to fetch contract");

    assert!(response.status().is_success(), "{}", response.status());

    let contract = response
        .json::<Contract>()
        .expect("failed to parse response");

    assert_eq!(contract.state.0, vec![4, 5, 6]);
}

#[test]
fn e2e() {
    tracing_subscriber::fmt::init();

    let path_node1 = Path::new("tests/node1");
    let path_node2 = Path::new("tests/node2");

    // Start 2 nodes
    let node1 = test_helpers::TestNode::new(path_node1, false, "6668");
    let node2 = test_helpers::TestNode::new(path_node2, false, "6669");

    // Wait for node to properly spin up
    thread::sleep(time::Duration::from_secs(1));

    // Request something on node1 to be sure it's alive and working
    let client = Client::new();

    register_contracts(&client);
    send_blobs(&client);
    // Wait for some slots to be finished
    thread::sleep(time::Duration::from_secs(5));
    verify_contract_state(&client);

    // Stop all processes
    drop(node1);
    drop(node2);

    // Check that some blocks has been produced
    let node1_consensus = Consensus::load_from_disk(path_node1).unwrap();
    let node2_consensus = Consensus::load_from_disk(path_node2).unwrap();
    assert!(!node1_consensus.blocks.is_empty());
    assert!(!node2_consensus.blocks.is_empty());
    // FIXME: check that created blocks are the same.

    // Clean created files
    fs::remove_dir_all(path_node1.join("data_node1")).expect("file cleaning failed");
    fs::remove_dir_all(path_node2.join("data_node2")).expect("file cleaning failed");

    fs::remove_file(path_node1.join("data.bin")).expect("file cleaning failed");
    fs::remove_file(path_node2.join("data.bin")).expect("file cleaning failed");

    //TODO: compare blocks from node1 and node2
}
