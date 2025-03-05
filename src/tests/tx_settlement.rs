use client_sdk::rest_client::{IndexerApiHttpClient, NodeApiHttpClient};
use hyle_model::{
    api::APIRegisterContract, BlobTransaction, ContractAction, ContractName, Hashed, OnchainEffect,
    ProgramId, ProofData, ProofTransaction, RegisterContractAction, RegisterContractEffect,
    StateDigest,
};
use testcontainers_modules::{
    postgres::Postgres,
    testcontainers::{runners::AsyncRunner, ImageExt},
};
use tracing::info;

use crate::{
    model::{Blob, BlobData},
    node_state::test::make_hyle_output_with_state,
    utils::integration_test::{NodeIntegrationCtx, NodeIntegrationCtxBuilder},
};

use anyhow::Result;

use hyle_contract_sdk::BlobIndex;

fn make_register_blob_action(
    contract_name: ContractName,
    state_digest: StateDigest,
) -> BlobTransaction {
    BlobTransaction::new(
        "hyle.hyle",
        vec![RegisterContractAction {
            verifier: "test".into(),
            program_id: ProgramId(vec![1, 2, 3]),
            state_digest,
            contract_name,
        }
        .as_blob("hyle".into(), None, None)],
    )
}

#[test_log::test(tokio::test)]
async fn test_full_settlement_flow() -> Result<()> {
    // Start postgres DB with default settings for the indexer.
    let pg = Postgres::default()
        .with_tag("17-alpine")
        .with_cmd(["postgres", "-c", "log_statement=all"])
        .start()
        .await
        .unwrap();

    let mut builder = NodeIntegrationCtxBuilder::new().await;
    let rest_client = builder.conf.rest.clone();
    builder.conf.run_indexer = true;
    builder.conf.database_url = format!(
        "postgres://postgres:postgres@localhost:{}/postgres",
        pg.get_host_port_ipv4(5432).await.unwrap()
    );
    let mut hyle_node = builder.build().await?;

    hyle_node.wait_for_genesis_event().await?;
    let client = NodeApiHttpClient::new(format!("http://{rest_client}/")).unwrap();
    hyle_node.wait_for_rest_api(&client).await?;

    info!("➡️  Registering contracts c1 & c2.hyle");

    let b1 = make_register_blob_action("c1".into(), StateDigest(vec![1, 2, 3]));
    client.send_tx_blob(&b1).await.unwrap();
    client
        .register_contract(&APIRegisterContract {
            verifier: "test".into(),
            program_id: ProgramId(vec![1, 2, 3]),
            state_digest: StateDigest(vec![7, 7, 7]),
            contract_name: "c2.hyle".into(),
        })
        .await
        .unwrap();

    info!("➡️  Sending blobs for c1 & c2.hyle");
    let tx = BlobTransaction::new(
        "test.c2.hyle",
        vec![
            Blob {
                contract_name: "c1".into(),
                data: BlobData(vec![0, 1, 2, 3]),
            },
            Blob {
                contract_name: "c2.hyle".into(),
                data: BlobData(vec![0, 1, 2, 3]),
            },
        ],
    );
    client.send_tx_blob(&tx).await.unwrap();

    info!("➡️  Sending proof for c1 & c2.hyle");
    let proof_c1 = ProofTransaction {
        contract_name: "c1".into(),
        proof: ProofData(
            borsh::to_vec(&vec![make_hyle_output_with_state(
                tx.clone(),
                BlobIndex(0),
                &[1, 2, 3],
                &[4, 5, 6],
            )])
            .unwrap(),
        ),
    };
    let proof_c2 = ProofTransaction {
        contract_name: "c2.hyle".into(),
        proof: ProofData(
            borsh::to_vec(&vec![make_hyle_output_with_state(
                tx.clone(),
                BlobIndex(1),
                &[7, 7, 7],
                &[8, 8, 8],
            )])
            .unwrap(),
        ),
    };
    client.send_tx_proof(&proof_c1).await.unwrap();
    client.send_tx_proof(&proof_c2).await.unwrap();

    info!("➡️  Waiting for TX to be settled");
    hyle_node.wait_for_settled_tx(tx.hashed()).await?;
    // Wait a block on top to make sure the state is updated.
    hyle_node.wait_for_n_blocks(1).await?;

    info!("➡️  Getting contracts");

    let contract = client.get_contract(&"c1".into()).await?;
    assert_eq!(contract.state.0, vec![4, 5, 6]);

    let contract = client.get_contract(&"c2.hyle".into()).await?;
    assert_eq!(contract.state.0, vec![8, 8, 8]);

    let pg_client = IndexerApiHttpClient::new(format!("http://{rest_client}/")).unwrap();
    let contract = pg_client.get_indexer_contract(&"c1".into()).await?;
    assert_eq!(contract.state_digest, vec![4, 5, 6]);

    let pg_client = IndexerApiHttpClient::new(format!("http://{rest_client}/")).unwrap();
    let contract = pg_client.get_indexer_contract(&"c2.hyle".into()).await?;
    assert_eq!(contract.state_digest, vec![8, 8, 8]);

    Ok(())
}

async fn build_hyle_node() -> Result<(String, NodeIntegrationCtx)> {
    // Start postgres DB with default settings for the indexer.
    let pg = Postgres::default()
        .with_tag("17-alpine")
        .with_cmd(["postgres", "-c", "log_statement=all"])
        .start()
        .await
        .unwrap();

    let mut builder = NodeIntegrationCtxBuilder::new().await;
    let rest_client = builder.conf.rest.clone();
    builder.conf.run_indexer = true;
    builder.conf.database_url = format!(
        "postgres://postgres:postgres@localhost:{}/postgres",
        pg.get_host_port_ipv4(5432).await.unwrap()
    );
    Ok((rest_client, builder.build().await?))
}

#[test_log::test(tokio::test)]
async fn test_tx_settlement_duplicates() -> Result<()> {
    let (rest_client, mut hyle_node) = build_hyle_node().await?;
    let client = NodeApiHttpClient::new(format!("http://{rest_client}/")).unwrap();

    hyle_node.wait_for_genesis_event().await?;
    hyle_node.wait_for_rest_api(&client).await?;

    let b1 = make_register_blob_action("c1".into(), StateDigest(vec![1, 2, 3]));

    info!("➡️  Registering contract c1 - first time");

    client.send_tx_blob(&b1).await.unwrap();
    hyle_node.wait_for_n_blocks(1).await?;

    info!("➡️  Registering contract c1 - second time");

    client.send_tx_blob(&b1).await.unwrap();
    hyle_node.wait_for_n_blocks(1).await?;

    client
        .register_contract(&APIRegisterContract {
            verifier: "test".into(),
            program_id: ProgramId(vec![1, 2, 3]),
            state_digest: StateDigest(vec![7, 7, 7]),
            contract_name: "c2.hyle".into(),
        })
        .await
        .unwrap();
    hyle_node.wait_for_n_blocks(1).await?;

    let tx = BlobTransaction::new(
        "test.c2.hyle",
        vec![
            Blob {
                contract_name: "c1".into(),
                data: BlobData(vec![0, 1, 2, 3]),
            },
            Blob {
                contract_name: "c2.hyle".into(),
                data: BlobData(vec![0, 1, 2, 3]),
            },
        ],
    );

    info!("➡️  Sending blobs for c1 & c2.hyle - first time");

    // Normal submit a blob tx for the first time
    client.send_tx_blob(&tx).await.unwrap();
    // Second time in the same block, should be dicarded
    client.send_tx_blob(&tx).await.unwrap();
    hyle_node.wait_for_n_blocks(1).await?;

    info!("➡️  Sending blobs for c1 & c2.hyle - second time");

    // Next block, the same tx should be discarded because its hash is already in the map
    client.send_tx_blob(&tx).await.unwrap();
    hyle_node.wait_for_n_blocks(1).await?;

    let proof_c1 = ProofTransaction {
        contract_name: "c1".into(),
        proof: ProofData(
            borsh::to_vec(&vec![make_hyle_output_with_state(
                tx.clone(),
                BlobIndex(0),
                &[1, 2, 3],
                &[4, 5, 6],
            )])
            .unwrap(),
        ),
    };
    let proof_c2 = ProofTransaction {
        contract_name: "c2.hyle".into(),
        proof: ProofData(
            borsh::to_vec(&vec![make_hyle_output_with_state(
                tx.clone(),
                BlobIndex(1),
                &[7, 7, 7],
                &[8, 8, 8],
            )])
            .unwrap(),
        ),
    };

    info!("➡️  Sending proof for c1 & c2.hyle - first time");
    // Normal - both should settle the blob txs
    client.send_tx_proof(&proof_c1).await.unwrap();
    client.send_tx_proof(&proof_c2).await.unwrap();

    info!("➡️  Sending proof for c1 & c2.hyle - second time");

    // Should fail, txs are already settled, no more to settle
    client.send_tx_proof(&proof_c1).await.unwrap();
    client.send_tx_proof(&proof_c2).await.unwrap();

    info!("➡️  Waiting for TX to be settled");
    hyle_node.wait_for_settled_tx(tx.hashed()).await?;

    let contract = client.get_contract(&"c1".into()).await?;
    assert_eq!(contract.state.0, vec![4, 5, 6]);
    let contract = client.get_contract(&"c2.hyle".into()).await?;
    assert_eq!(contract.state.0, vec![8, 8, 8]);

    // Recompute proofs to transition to next state

    let proof_c1 = ProofTransaction {
        contract_name: "c1".into(),
        proof: ProofData(
            borsh::to_vec(&vec![make_hyle_output_with_state(
                tx.clone(),
                BlobIndex(0),
                &[4, 5, 6],
                &[7, 8, 9],
            )])
            .unwrap(),
        ),
    };
    let proof_c2 = ProofTransaction {
        contract_name: "c2.hyle".into(),
        proof: ProofData(
            borsh::to_vec(&vec![make_hyle_output_with_state(
                tx.clone(),
                BlobIndex(1),
                &[8, 8, 8],
                &[9, 9, 9],
            )])
            .unwrap(),
        ),
    };

    info!("➡️  Sending proof for c1 & c2.hyle  v2 - first time");

    // Should fail, the extra blob tx should have been discarded, and not in an unsettled state
    client.send_tx_proof(&proof_c1).await.unwrap();
    client.send_tx_proof(&proof_c2).await.unwrap();

    info!("➡️  Sending blobs for c1 & c2.hyle - 3rd time");

    // Re submit the same blob tx after it was settled - should be accepted

    client.send_tx_blob(&tx).await.unwrap();
    hyle_node.wait_for_n_blocks(1).await?;

    let contract = client.get_contract(&"c1".into()).await?;
    assert_eq!(contract.state.0, vec![4, 5, 6]);
    let contract = client.get_contract(&"c2.hyle".into()).await?;
    assert_eq!(contract.state.0, vec![8, 8, 8]);

    info!("➡️  Sending proof for c1 & c2.hyle  v2 - second time");

    // Same blob is now in an unsettled state, proofs are up to date with the contract states
    client.send_tx_proof(&proof_c1).await.unwrap();
    client.send_tx_proof(&proof_c2).await.unwrap();

    info!("➡️  Waiting for TX to be settled - second time");
    hyle_node.wait_for_settled_tx(tx.hashed()).await?;
    // Wait a block on top to make sure the state is updated.
    hyle_node.wait_for_n_blocks(1).await?;

    info!("➡️  Getting contracts");

    let contract = client.get_contract(&"c1".into()).await?;
    assert_eq!(contract.state.0, vec![7, 8, 9]);

    let contract = client.get_contract(&"c2.hyle".into()).await?;
    assert_eq!(contract.state.0, vec![9, 9, 9]);

    // tokio::time::sleep(Duration::from_secs(3)).await;

    // let pg_client = IndexerApiHttpClient::new(format!("http://{rest_client}/")).unwrap();
    // let contract = pg_client.get_indexer_contract(&"c1".into()).await?;
    // assert_eq!(contract.state_digest, vec![7, 8, 9]);

    // let pg_client = IndexerApiHttpClient::new(format!("http://{rest_client}/")).unwrap();
    // let contract = pg_client.get_indexer_contract(&"c2.hyle".into()).await?;
    // assert_eq!(contract.state_digest, vec![9, 9, 9]);

    Ok(())
}

#[test_log::test(tokio::test)]
async fn test_contract_upgrade() -> Result<()> {
    let builder = NodeIntegrationCtxBuilder::new().await;
    let rest_url = builder.conf.rest.clone();
    let mut hyle_node = builder.build().await?;

    hyle_node.wait_for_genesis_event().await?;

    let client = NodeApiHttpClient::new(format!("http://{rest_url}/")).unwrap();
    hyle_node.wait_for_rest_api(&client).await?;

    info!("➡️  Registering contracts c1.hyle");

    let b1 = make_register_blob_action("c1.hyle".into(), StateDigest(vec![1, 2, 3]));
    client.send_tx_blob(&b1).await.unwrap();

    hyle_node.wait_for_settled_tx(b1.hashed()).await?;

    let contract = client.get_contract(&"c1.hyle".into()).await?;
    assert_eq!(contract.program_id, ProgramId(vec![1, 2, 3]));

    // Send contract update transaction
    let b2 = BlobTransaction::new(
        "toto.c1.hyle",
        vec![Blob {
            contract_name: "c1.hyle".into(),
            data: BlobData(vec![1]),
        }],
    );
    client.send_tx_blob(&b2).await.unwrap();

    let mut hyle_output =
        make_hyle_output_with_state(b2.clone(), BlobIndex(0), &[1, 2, 3], &[8, 8, 8]);

    hyle_output
        .onchain_effects
        .push(OnchainEffect::RegisterContract(RegisterContractEffect {
            verifier: "test".into(),
            program_id: ProgramId(vec![7, 7, 7]),
            // The state digest is ignored during the update phase.
            state_digest: StateDigest(vec![3, 3, 3]),
            contract_name: "c1.hyle".into(),
        }));

    let proof_update = ProofTransaction {
        contract_name: "c1.hyle".into(),
        proof: ProofData(borsh::to_vec(&vec![hyle_output]).unwrap()),
    };

    client.send_tx_proof(&proof_update).await.unwrap();

    info!("➡️  Waiting for TX to be settled");
    hyle_node.wait_for_settled_tx(b2.hashed()).await?;
    // Wait a block on top to make sure the state is updated.
    hyle_node.wait_for_n_blocks(1).await?;

    info!("➡️  Getting contracts");

    let contract = client.get_contract(&"c1.hyle".into()).await?;
    // Check program ID has changed.
    assert_eq!(contract.program_id, ProgramId(vec![7, 7, 7]));
    // We use the next_state, not the state_digest of the update effect.
    assert_eq!(contract.state.0, vec![8, 8, 8]);

    Ok(())
}
