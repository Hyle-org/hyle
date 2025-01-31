use client_sdk::rest_client::{IndexerApiHttpClient, NodeApiHttpClient};
use hyle_model::{
    api::APIRegisterContract, BlobTransaction, ContractAction, ContractName, Hashable, ProgramId,
    ProofData, ProofTransaction, RegisterContractAction, RegisterContractEffect, StateDigest,
};
use testcontainers_modules::{
    postgres::Postgres,
    testcontainers::{runners::AsyncRunner, ImageExt},
};
use tracing::info;

use crate::{
    model::{Blob, BlobData},
    node_state::test::make_hyle_output_with_state,
    utils::integration_test::NodeIntegrationCtxBuilder,
};

use anyhow::Result;

use hyle_contract_sdk::BlobIndex;

fn make_register_blob_action(
    contract_name: ContractName,
    state_digest: StateDigest,
) -> BlobTransaction {
    BlobTransaction {
        identity: "hyle.hyle".into(),
        blobs: vec![RegisterContractAction {
            verifier: "test".into(),
            program_id: ProgramId(vec![1, 2, 3]),
            state_digest,
            contract_name,
        }
        .as_blob("hyle".into(), None, None)],
    }
}

#[test_log::test(tokio::test)]
async fn test_full_settlement_flow() -> Result<()> {
    // Start postgres DB with default settings for the indexer.
    let pg = Postgres::default()
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
    let tx = BlobTransaction {
        identity: "test.c2.hyle".into(),
        blobs: vec![
            Blob {
                contract_name: "c1".into(),
                data: BlobData(vec![0, 1, 2, 3]),
            },
            Blob {
                contract_name: "c2.hyle".into(),
                data: BlobData(vec![0, 1, 2, 3]),
            },
        ],
    };
    client.send_tx_blob(&tx).await.unwrap();

    info!("➡️  Sending proof for c1 & c2.hyle");
    let proof_c1 = ProofTransaction {
        contract_name: "c1".into(),
        proof: ProofData(
            bincode::encode_to_vec(
                vec![make_hyle_output_with_state(
                    tx.clone(),
                    BlobIndex(0),
                    &[1, 2, 3],
                    &[4, 5, 6],
                )],
                bincode::config::standard(),
            )
            .unwrap(),
        ),
    };
    let proof_c2 = ProofTransaction {
        contract_name: "c2.hyle".into(),
        proof: ProofData(
            bincode::encode_to_vec(
                vec![make_hyle_output_with_state(
                    tx.clone(),
                    BlobIndex(1),
                    &[7, 7, 7],
                    &[8, 8, 8],
                )],
                bincode::config::standard(),
            )
            .unwrap(),
        ),
    };
    client.send_tx_proof(&proof_c1).await.unwrap();
    client.send_tx_proof(&proof_c2).await.unwrap();

    info!("➡️  Waiting for TX to be settled");
    hyle_node.wait_for_settled_tx(tx.hash()).await?;
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

#[test_log::test(tokio::test)]
async fn test_contract_upgrade() -> Result<()> {
    let builder = NodeIntegrationCtxBuilder::new().await;
    let rest_url = builder.conf.rest.clone();
    let mut hyle_node = builder.build().await?;

    hyle_node.wait_for_genesis_event().await?;

    let client = NodeApiHttpClient::new(format!("http://{rest_url}/")).unwrap();

    info!("➡️  Registering contracts c1.hyle");

    let b1 = make_register_blob_action("c1.hyle".into(), StateDigest(vec![1, 2, 3]));
    client.send_tx_blob(&b1).await.unwrap();

    hyle_node.wait_for_settled_tx(b1.hash()).await?;

    let contract = client.get_contract(&"c1.hyle".into()).await?;
    assert_eq!(contract.program_id, ProgramId(vec![1, 2, 3]));

    // Send contract update transaction
    let b2 = BlobTransaction {
        identity: "toto.c1.hyle".into(),
        blobs: vec![Blob {
            contract_name: "c1.hyle".into(),
            data: BlobData(vec![1]),
        }],
    };
    client.send_tx_blob(&b2).await.unwrap();

    let mut hyle_output =
        make_hyle_output_with_state(b2.clone(), BlobIndex(0), &[1, 2, 3], &[8, 8, 8]);

    hyle_output
        .registered_contracts
        .push(RegisterContractEffect {
            verifier: "test".into(),
            program_id: ProgramId(vec![7, 7, 7]),
            // The state digest is ignored during the update phase.
            state_digest: StateDigest(vec![3, 3, 3]),
            contract_name: "c1.hyle".into(),
        });

    let proof_update = ProofTransaction {
        contract_name: "c1.hyle".into(),
        proof: ProofData(
            bincode::encode_to_vec(vec![hyle_output], bincode::config::standard()).unwrap(),
        ),
    };

    client.send_tx_proof(&proof_update).await.unwrap();

    info!("➡️  Waiting for TX to be settled");
    hyle_node.wait_for_settled_tx(b2.hash()).await?;
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
