#![allow(clippy::unwrap_used, clippy::expect_used)]
use fixtures::{contracts::TestContract, ctx::E2ECtx};
use tracing::info;

use hyle::model::{Blob, BlobData};

mod fixtures;

use anyhow::Result;

mod e2e_tx_settle {
    use hyle_contract_sdk::BlobIndex;

    use super::*;

    async fn scenario_test_settlement(ctx: E2ECtx) -> Result<()> {
        info!("➡️  Registering contracts c1 & c2");
        ctx.register_contract::<TestContract>("c1").await?;
        ctx.register_contract::<TestContract>("c2").await?;

        info!("➡️  Sending blobs for c1 & c2");
        let blobs = vec![
            Blob {
                contract_name: "c1".into(),
                data: BlobData(vec![0, 1, 2, 3]),
            },
            Blob {
                contract_name: "c2".into(),
                data: BlobData(vec![0, 1, 2, 3]),
            },
        ];
        let blob_tx_hash = ctx.send_blob("test.c1".into(), blobs.clone()).await?;

        info!("➡️  Sending proof for c1 & c2");
        let proof_c1 = ctx.make_proof::<TestContract>(
            "test.c1".into(),
            blobs.clone(),
            &blob_tx_hash,
            BlobIndex(0),
        );
        let proof_c2 = ctx.make_proof::<TestContract>(
            "test.c1".into(),
            blobs.clone(),
            &blob_tx_hash,
            BlobIndex(1),
        );
        ctx.send_proof_single("c1".into(), proof_c1, blob_tx_hash.clone())
            .await?;
        ctx.send_proof_single("c2".into(), proof_c2, blob_tx_hash)
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

    #[test_log::test(tokio::test)]
    async fn on_indexer() -> Result<()> {
        let ctx = E2ECtx::new_multi_with_indexer(2, 500).await?;

        scenario_test_settlement(ctx).await
    }
}
