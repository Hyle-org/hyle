use client_sdk::rest_client::{IndexerApiHttpClient, NodeApiHttpClient};
use hyle_model::{
    BlobTransaction, ContractAction, ContractName, Hashable, ProgramId, ProofData,
    ProofTransaction, RegisterContractAction, StateDigest,
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
    let mut node_modules = builder.build().await?;

    node_modules.wait_for_genesis_event().await?;

    let client = NodeApiHttpClient::new(format!("http://{rest_client}/")).unwrap();

    info!("➡️  Registering contracts c1 & c2.hyle");

    let b1 = make_register_blob_action("c1".into(), StateDigest(vec![1, 2, 3]));
    let b2 = make_register_blob_action("c2.hyle".into(), StateDigest(vec![7, 7, 7]));

    client.send_tx_blob(&b1).await.unwrap();
    client.send_tx_blob(&b2).await.unwrap();

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
    node_modules.wait_for_settled_tx(tx.hash()).await?;
    // Wait a block on top to make sure the state is updated.
    node_modules.wait_for_n_blocks(1).await?;

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
