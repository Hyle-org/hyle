use hyle::{
    model::{
        Blob, BlobData, BlobReference, BlobTransaction, ContractName, Identity, ProofTransaction,
        RegisterContractTransaction, StateDigest,
    },
    node_state::model::Contract,
};
use reqwest::blocking::{Client, Response};
use serde::Serialize;
use std::{thread, time};

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

#[test]
fn e2e_contract_state_updated() {
    tracing_subscriber::fmt::init();

    // Start first node
    let node1 = test_helpers::TestNode::new("master.ron", false);
    // Start second node
    // let node2 = test_helpers::TestNode::new("1", "config.ron", false);

    // Wait for server to properly start
    thread::sleep(time::Duration::from_secs(1));

    // Request something on node1 to be sure it's alive and working
    let client = Client::new();

    assert!(send(
        &client,
        url("/v1/contract/register"),
        RegisterContractTransaction {
            owner: "test".to_string(),
            verifier: "yoloo".to_string(),
            program_id: vec![1, 2, 3],
            state_digest: StateDigest::default(),
            contract_name: ContractName("c1".to_string()),
        },
    )
    .status()
    .is_success());

    assert!(send(
        &client,
        url("/v1/contract/register"),
        RegisterContractTransaction {
            owner: "test".to_string(),
            verifier: "yoloo".to_string(),
            program_id: vec![1, 2, 3],
            state_digest: StateDigest::default(),
            contract_name: ContractName("c2".to_string()),
        },
    )
    .status()
    .is_success());

    assert!(send(
        &client,
        url("/v1/tx/send/blob"),
        BlobTransaction {
            identity: Identity("client".to_string()),
            blobs: vec![
                Blob {
                    contract_name: ContractName("c1".to_string()),
                    data: BlobData(vec![0, 1, 2, 3])
                },
                Blob {
                    contract_name: ContractName("c2".to_string()),
                    data: BlobData(vec![0, 1, 2, 3])
                }
            ]
        },
    )
    .status()
    .is_success());

    assert!(send(
        &client,
        url("/v1/tx/send/proof"),
        ProofTransaction {
            blobs_references: vec![
                BlobReference {
                    contract_name: ContractName("c1".to_string()),
                    blob_tx_hash: serde_json::from_str(
                        "\"d3b6fa8ff25fab0209f821530b9a138f72c757be5828ee40128072a592817eab\""
                    )
                    .unwrap(),
                    blob_index: hyle::model::BlobIndex(0)
                },
                BlobReference {
                    contract_name: ContractName("c2".to_string()),
                    blob_tx_hash: serde_json::from_str(
                        "\"d3b6fa8ff25fab0209f821530b9a138f72c757be5828ee40128072a592817eab\""
                    )
                    .unwrap(),
                    blob_index: hyle::model::BlobIndex(1)
                }
            ],
            proof: vec![5, 5]
        },
    )
    .status()
    .is_success());

    // wait for new block to be emitted
    std::thread::sleep(time::Duration::from_secs(10));

    let response = client
        .get(url("/v1/contract/c1"))
        .header("Content-Type", "application/json")
        .send()
        .expect("Failed to fetch contract");

    assert!(response.status().is_success());

    let contract = response
        .json::<Contract>()
        .expect("failed to parse response");

    assert_eq!(contract.state.0, vec![4, 5, 6]);

    // Stop all processes
    drop(node1);
}
