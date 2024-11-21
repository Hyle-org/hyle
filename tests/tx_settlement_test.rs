use fixtures::{
    contracts::{ERC20Contract, TestContract},
    ctx::E2ECtx,
};
use std::{fs::File, io::Read};
use tracing::info;

use hyle::model::{Blob, BlobData, BlobReference, ContractName, ProofData};

mod fixtures;

use anyhow::Result;

pub fn load_encoded_receipt_from_file(path: &str) -> Vec<u8> {
    let mut file = File::open(path).expect("Failed to open proof file");
    let mut encoded_receipt = Vec::new();
    file.read_to_end(&mut encoded_receipt)
        .expect("Failed to read file content");
    encoded_receipt
}

mod e2e_tx_settle {
    use super::*;

    async fn scenario_test_settlement(ctx: E2ECtx) -> Result<()> {
        info!("➡️  Registering contracts c1 & c2");
        ctx.register_contract::<TestContract>("c1").await?;
        ctx.register_contract::<TestContract>("c2").await?;

        info!("➡️  Sending blobs for c1 & c2");
        let blob_tx_hash = ctx
            .send_blob(
                "test".into(),
                vec![
                    Blob {
                        contract_name: "c1".into(),
                        data: BlobData(vec![0, 1, 2, 3]),
                    },
                    Blob {
                        contract_name: "c2".into(),
                        data: BlobData(vec![0, 1, 2, 3]),
                    },
                ],
            )
            .await?;

        info!("➡️  Sending proof for c1 & c2");
        ctx.send_proof(
            vec![
                BlobReference {
                    contract_name: "c1".into(),
                    blob_tx_hash: blob_tx_hash.clone(),
                    blob_index: hyle_contract_sdk::BlobIndex(0),
                },
                BlobReference {
                    contract_name: "c2".into(),
                    blob_tx_hash,
                    blob_index: hyle_contract_sdk::BlobIndex(1),
                },
            ],
            ProofData::Bytes(vec![5, 5]),
        )
        .await?;

        info!("➡️  Waiting for height 2");
        ctx.wait_height(2).await?;

        info!("➡️  Getting contracts");
        let contract = ctx.get_contract("c1").await?;
        assert_eq!(contract.state.0, vec![4, 5, 6]);

        if ctx.has_indexer() {
            let contract = ctx.get_indexer_contract("c1").await?;
            assert_eq!(contract.state_digest, vec![4, 5, 6]);
        }
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn single() -> Result<()> {
        let ctx = E2ECtx::new_single(500).await?;

        scenario_test_settlement(ctx).await
    }

    #[test_log::test(tokio::test)]
    async fn multi() -> Result<()> {
        let ctx = E2ECtx::new_multi(2, 500).await?;

        scenario_test_settlement(ctx).await
    }

    #[ignore = "Indexer do not implement contract state updated yet"]
    #[test_log::test(tokio::test)]
    async fn on_indexer() -> Result<()> {
        let ctx = E2ECtx::new_multi_with_indexer(2, 500).await?;

        scenario_test_settlement(ctx).await
    }

    async fn scenario_erc20(ctx: E2ECtx) -> Result<()> {
        info!("➡️  Registering contract erc20-risc0");
        ctx.register_contract::<ERC20Contract>("erc20-risc0")
            .await?;

        info!("➡️  Sending blobs for erc20-risc0");
        let blob_tx_hash = ctx
            .send_blob(
                "test".into(),
                vec![Blob {
                    contract_name: "erc20-risc0".into(),
                    data: BlobData(vec![1, 3, 109, 97, 120, 27]),
                }],
            )
            .await?;

        let proof = load_encoded_receipt_from_file("./tests/proofs/erc20.risc0.proof");

        info!("➡️  Sending proof for erc20-risc0");
        ctx.send_proof(
            vec![BlobReference {
                contract_name: ContractName("erc20-risc0".to_string()),
                blob_tx_hash: blob_tx_hash.clone(),
                blob_index: hyle_contract_sdk::BlobIndex(0),
            }],
            ProofData::Bytes(proof),
        )
        .await?;

        info!("➡️  Waiting for height 5");
        ctx.wait_height(5).await?;

        let contract = ctx.get_contract("erc20-risc0").await?;
        assert_eq!(
            contract.state.0,
            vec![
                154, 65, 139, 95, 54, 114, 201, 168, 66, 153, 34, 153, 43, 237, 17, 198, 0, 39, 64,
                81, 204, 183, 209, 41, 84, 147, 193, 217, 48, 42, 213, 57
            ]
        );

        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn risc0_single_node() -> Result<()> {
        let ctx = E2ECtx::new_single(500).await?;
        scenario_erc20(ctx).await
    }

    #[test_log::test(tokio::test)]
    async fn risc0_multi_nodes() -> Result<()> {
        let ctx = E2ECtx::new_multi(2, 500).await?;

        scenario_erc20(ctx).await
    }
}
