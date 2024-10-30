use std::time::Duration;

use anyhow::Result;
use fixtures::{contracts::TestContract, ctx::E2ECtx};
use tokio::time::sleep;

mod fixtures;

mod e2e_consensus {
    use hyle::consensus::staking::{Stake, Staker};

    use super::*;

    #[test_log::test(tokio::test)]
    async fn single_node_generates_blocks() -> Result<()> {
        let ctx = E2ECtx::new_single(50).await?;

        // TODO remove these 3 lines when the single node is fixed
        // at the moment, the single node needs 2 transactions to start generating blocks

        sleep(Duration::from_secs(2)).await;
        ctx.register_contract::<TestContract>("c1").await?;
        ctx.register_contract::<TestContract>("c2").await?;

        ctx.wait_height(20).await?;

        Ok(())
    }

    #[ignore = "broken for now"]
    #[test_log::test(tokio::test)]
    async fn can_run_lot_of_nodes() -> Result<()> {
        let ctx = E2ECtx::new_multi(10, 500).await?;

        ctx.wait_height(2).await?;

        Ok(())
    }

    #[ignore = "broken for now"]
    #[test_log::test(tokio::test)]
    async fn can_rejoin() -> Result<()> {
        let mut ctx = E2ECtx::new_multi(2, 500).await?;

        ctx.wait_height(2).await?;

        let joining_client = ctx.add_node().await?;

        let node_info = joining_client.get_node_info().await?;

        assert!(node_info.pubkey.is_some());

        ctx.client()
            .send_stake_tx(&Staker {
                pubkey: node_info.pubkey.clone().unwrap(),
                stake: Stake { amount: 100 },
            })
            .await?;

        // 2 slots to get the tx in a blocks
        // 1 slot to send the candidacy
        // 1 slot to add the validator to consensus
        ctx.wait_height(4).await?;

        let consensus = ctx.client().get_consensus_info().await?;

        assert_eq!(consensus.validators.len(), 3, "expected 3 validators");

        assert!(
            consensus.validators.contains(&node_info.pubkey.unwrap()),
            "node pubkey not found in validators",
        );

        Ok(())
    }
}
