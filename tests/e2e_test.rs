use hyle::rest::model::TransactionRequest;
use reqwest::blocking::Client;
use std::{thread, time};

mod test_helpers;

#[test]
fn test_e2e_with_cli() {
    tracing_subscriber::fmt::init();

    // Start first node
    let node1 = test_helpers::TestNode::new("master.ron", false);
    // Start second node
    // let node2 = test_helpers::TestNode::new("1", "config.ron", false);

    // Wait for server to properly start
    thread::sleep(time::Duration::from_secs(1));

    // Start client that connects to node1
    let client1 = test_helpers::TestNode::new("master.ron", true);

    // Request something on node1 to be sure it's alive and working
    let client = Client::new();
    let url = "http://127.0.0.1:4321/getTransaction";
    let tx = TransactionRequest {
        tx_hash: "h".to_owned(),
    };
    let request_body = serde_json::json!(tx);

    let response = client
        .get(url)
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .expect("Failed to send request");

    assert!(response.status().is_success());

    // Stop all processes
    drop(node1);
    // drop(node2);
    drop(client1);
}