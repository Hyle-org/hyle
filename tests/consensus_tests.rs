use anyhow::Result;
use fixtures::ctx::E2ECtx;

mod fixtures;

mod e2e_consensus {

    use client_sdk::transaction_builder::TransactionBuilder;
    use fixtures::test_helpers::send_transaction;
    use hyle::{genesis::States, utils::logger::LogMe};
    use hyle_contract_sdk::Identity;
    use staking::state::OnChainState;

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

    #[test_log::test(tokio::test)]
    async fn can_rejoin() -> Result<()> {
        let mut ctx = E2ECtx::new_multi_with_indexer(2, 500).await?;

        ctx.wait_height(4).await?;

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

        let staking = ctx.client().get_consensus_staking_state().await.unwrap();

        assert_eq!(staking_state, staking.on_chain_state());
        let mut states = States {
            hyllar,
            hydentity,
            staking,
        };

        let node_identity = Identity(format!("{}.hydentity", node_info.id));
        {
            let mut transaction = TransactionBuilder::new(node_identity.clone());

            states
                .hydentity
                .default_builder(&mut transaction)
                .register_identity("password".to_string())?;

            send_transaction(ctx.client(), transaction, &mut states).await;
        }
        {
            let mut transaction = TransactionBuilder::new("faucet.hydentity".into());

            states
                .hydentity
                .default_builder(&mut transaction)
                .verify_identity(&states.hydentity, "password".to_string())?;
            states
                .hyllar
                .default_builder(&mut transaction)
                .transfer(node_identity.0.clone(), 100)?;

            send_transaction(ctx.client(), transaction, &mut states).await;
        }
        {
            let mut transaction = TransactionBuilder::new(node_identity.clone());

            states
                .hydentity
                .default_builder(&mut transaction)
                .verify_identity(&states.hydentity, "password".to_string())?;
            states.staking.builder(&mut transaction).stake(100)?;
            states
                .hyllar
                .default_builder(&mut transaction)
                .transfer("staking".to_string(), 100)?;
            states
                .staking
                .builder(&mut transaction)
                .delegate(node_info.pubkey.clone().unwrap())?;

            send_transaction(ctx.client(), transaction, &mut states).await;
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
}
