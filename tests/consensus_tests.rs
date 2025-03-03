#![allow(clippy::unwrap_used, clippy::expect_used)]
use anyhow::Result;
use fixtures::ctx::E2ECtx;

mod fixtures;

mod e2e_consensus {

    use client_sdk::helpers::risc0::Risc0Prover;
    use client_sdk::transaction_builder::{ProvableBlobTx, TxExecutorBuilder};
    use fixtures::test_helpers::send_transaction;
    use hydentity::client::{register_identity, verify_identity};
    use hydentity::Hydentity;
    use hyle::{genesis::States, utils::logger::LogMe};
    use hyle_contract_sdk::Digestable;
    use hyle_contract_sdk::Identity;
    use hyle_contracts::{HYDENTITY_ELF, HYLLAR_ELF, STAKING_ELF};
    use hyle_model::{ContractName, StateDigest};
    use hyllar::client::transfer;
    use hyllar::{Hyllar, FAUCET_ID};
    use staking::client::{delegate, stake};
    use staking::state::Staking;
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

    async fn scenario_rejoin_common(ctx: &mut E2ECtx, stake_amount: u128) -> Result<()> {
        ctx.wait_height(2).await?;

        let joining_client = ctx.add_node().await?;

        let node_info = joining_client.get_node_info().await?;

        assert!(node_info.pubkey.is_some());

        let hyllar: Hyllar = ctx
            .indexer_client()
            .fetch_current_state(&"hyllar".into())
            .await
            .log_error("fetch state failed")
            .unwrap();
        let hydentity: Hydentity = StateDigest(
            ctx.indexer_client()
                .get_indexer_contract(&"hydentity".into())
                .await
                .unwrap()
                .state_digest,
        )
        .try_into()
        .unwrap();

        let staking_state: StateDigest = StateDigest(
            ctx.indexer_client()
                .get_indexer_contract(&"staking".into())
                .await?
                .state_digest,
        );

        let staking: Staking = ctx
            .client()
            .get_consensus_staking_state()
            .await
            .unwrap()
            .into();

        assert_eq!(staking_state, staking.as_digest());
        let states = States {
            hyllar,
            hydentity,
            staking,
        };

        let mut tx_ctx = TxExecutorBuilder::new(states)
            // Replace prover binaries for non-reproducible mode.
            .with_prover("hydentity".into(), Risc0Prover::new(HYDENTITY_ELF))
            .with_prover("hyllar".into(), Risc0Prover::new(HYLLAR_ELF))
            .with_prover("staking".into(), Risc0Prover::new(STAKING_ELF))
            .build();

        let node_identity = Identity(format!("{}.hydentity", node_info.id));
        {
            let mut transaction = ProvableBlobTx::new(node_identity.clone());

            register_identity(&mut transaction, "hydentity".into(), "password".to_owned())?;

            let tx_hash = send_transaction(ctx.client(), transaction, &mut tx_ctx).await;
            tracing::warn!("Register TX Hash: {:?}", tx_hash);
        }
        {
            let mut transaction = ProvableBlobTx::new(FAUCET_ID.into());

            verify_identity(
                &mut transaction,
                "hydentity".into(),
                &tx_ctx.hydentity,
                "password".to_string(),
            )
            .expect("verify_identity failed");

            transfer(
                &mut transaction,
                "hyllar".into(),
                node_identity.0.clone(),
                stake_amount,
            )
            .expect("transfer failed");

            let tx_hash = send_transaction(ctx.client(), transaction, &mut tx_ctx).await;
            tracing::warn!("Transfer TX Hash: {:?}", tx_hash);
        }
        {
            let mut transaction = ProvableBlobTx::new(node_identity.clone());

            verify_identity(
                &mut transaction,
                "hydentity".into(),
                &tx_ctx.hydentity,
                "password".to_string(),
            )?;

            stake(&mut transaction, ContractName::new("staking"), stake_amount)?;

            transfer(
                &mut transaction,
                "hyllar".into(),
                "staking".to_string(),
                stake_amount,
            )?;

            delegate(&mut transaction, node_info.pubkey.clone().unwrap())?;

            let tx_hash = send_transaction(ctx.client(), transaction, &mut tx_ctx).await;
            tracing::warn!("staking TX Hash: {:?}", tx_hash);
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

        scenario_rejoin_common(&mut ctx, 100).await?;

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

        scenario_rejoin_common(&mut ctx, 50).await?;

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
