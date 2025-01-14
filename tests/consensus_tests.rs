#![allow(clippy::unwrap_used, clippy::expect_used)]
use anyhow::Result;
use fixtures::ctx::E2ECtx;

mod fixtures;

mod e2e_consensus {

    use client_sdk::transaction_builder::{ProvableBlobTx, TxExecutorBuilder};
    use fixtures::test_helpers::send_transaction;
    use hydentity::client::{register_identity, verify_identity};
    use hyle::{genesis::States, utils::logger::LogMe};
    use hyle_contract_sdk::Identity;
    use hyllar::client::transfer;
    use staking::client::{delegate, stake};
    use staking::state::{OnChainState, Staking};
    use tracing::info;

    use super::*;

    #[test_log::test(tokio::test)]
    async fn single_node_generates_blocks() -> Result<()> {
        let ctx = E2ECtx::new_single(50).await?;
        ctx.wait_height(20).await?;
        Ok(())
    }

    #[ignore = "flakky"]
    #[test_log::test(tokio::test)]
    async fn can_run_lot_of_nodes() -> Result<()> {
        let ctx = E2ECtx::new_multi(10, 1000).await?;

        ctx.wait_height(2).await?;

        Ok(())
    }

    async fn scenario_rejoin_common(ctx: &mut E2ECtx) -> Result<()> {
        ctx.wait_height(2).await?;

        let joining_client = ctx.add_node().await?;

        let node_info = joining_client.get_node_info().await?;

        assert!(node_info.pubkey.is_some());

        let hyllar = ctx
            .indexer_client()
            .fetch_current_state(&"hyllar".into())
            .await
            .log_error("fetch state failed")
            .unwrap();
        let hydentity = ctx
            .indexer_client()
            .fetch_current_state(&"hydentity".into())
            .await
            .unwrap();

        let staking_state: OnChainState = ctx
            .indexer_client()
            .fetch_current_state(&"staking".into())
            .await
            .unwrap();

        let staking: Staking = ctx
            .client()
            .get_consensus_staking_state()
            .await
            .unwrap()
            .into();

        assert_eq!(staking_state, staking.on_chain_state());
        let states = States {
            hyllar,
            hydentity,
            staking,
        };

        let mut tx_ctx = TxExecutorBuilder::default().with_state(states);

        let node_identity = Identity(format!("{}.hydentity", node_info.id));
        {
            let mut transaction = ProvableBlobTx::new(node_identity.clone());

            register_identity(&mut transaction, "hydentity".into(), "password".to_owned())?;

            send_transaction(ctx.client(), transaction, &mut tx_ctx).await;
        }
        {
            let mut transaction = ProvableBlobTx::new("faucet.hydentity".into());

            verify_identity(
                &mut transaction,
                "hydentity".into(),
                &tx_ctx.hydentity,
                "password".to_string(),
            )?;

            transfer(
                &mut transaction,
                "hyllar".into(),
                node_identity.0.clone(),
                100,
            )?;

            send_transaction(ctx.client(), transaction, &mut tx_ctx).await;
        }
        {
            let mut transaction = ProvableBlobTx::new(node_identity.clone());

            verify_identity(
                &mut transaction,
                "hydentity".into(),
                &tx_ctx.hydentity,
                "password".to_string(),
            )?;

            stake(&mut transaction, "staking".into(), 100)?;

            transfer(
                &mut transaction,
                "hyllar".into(),
                "staking".to_string(),
                100,
            )?;

            delegate(
                &mut transaction,
                "staking".into(),
                node_info.pubkey.clone().unwrap(),
            )?;

            send_transaction(ctx.client(), transaction, &mut tx_ctx).await;
        }

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

        ctx.wait_height(1).await?;
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn can_rejoin_blocking_consensus() -> Result<()> {
        let mut ctx = E2ECtx::new_multi_with_indexer(2, 500).await?;

        scenario_rejoin_common(&mut ctx).await?;

        // TODO: we should be able to exit the consensus and rejoin it again, but this doesn't work when we block it for now.
        /*
        info!("Stopping node");
        ctx.stop_node(3).await?;

        // Wait for a few seconds
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

        info!("Experienced timeouts");
        //info!("Waiting for a few blocks");
        //ctx.wait_height(2).await?;

        // We restart during the timeout timer which is fine.
        ctx.restart_node(3)?;

        ctx.wait_height(4).await?;
        */
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn can_rejoin_not_blocking_consensus() -> Result<()> {
        let mut ctx = E2ECtx::new_multi_with_indexer(2, 500).await?;

        scenario_rejoin_common(&mut ctx).await?;

        info!("Stopping node");
        ctx.stop_node(3).await?;

        info!("Waiting for a few blocks");
        ctx.wait_height(2).await?;

        // We restart during the timeout timer which is fine.
        ctx.restart_node(3)?;

        ctx.wait_height(1).await?;

        Ok(())
    }
}
