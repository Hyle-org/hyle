#![allow(clippy::unwrap_used, clippy::expect_used)]
use fixtures::ctx::E2ECtx;
use tracing::info;

mod fixtures;

use anyhow::Result;

mod e2e_hyllar {
    use client_sdk::{
        contract_states,
        transaction_builder::{ProvableBlobTx, TxExecutorBuilder},
    };
    use hydentity::{
        client::{register_identity, verify_identity},
        Hydentity,
    };
    use hyle_contract_sdk::{erc20::ERC20, ContractName, Digestable, StateDigest};
    use hyllar::{client::transfer, HyllarToken};

    use super::*;

    contract_states!(
        struct States {
            hydentity: Hydentity,
            hyllar: HyllarToken,
        }
    );

    async fn scenario_hyllar(ctx: E2ECtx) -> Result<()> {
        info!("➡️  Setting up the executor with the initial state");

        let contract = ctx.get_contract("hydentity").await?;
        let hydentity: hydentity::Hydentity = contract.state.try_into()?;
        let contract = ctx.get_contract("hyllar").await?;
        let hyllar: HyllarToken = contract.state.try_into()?;
        let mut executor = TxExecutorBuilder::default().with_state(States { hydentity, hyllar });

        info!("➡️  Sending blob to register bob identity");

        let mut tx = ProvableBlobTx::new("bob.hydentity".into());
        register_identity(&mut tx, "hydentity".into(), "password".to_string())?;
        let blob_tx_hash = ctx.send_provable_blob_tx(&tx).await?;

        let tx = executor.process(tx)?;
        let proof = tx.iter_prove().next().unwrap().0.await?;

        info!("➡️  Sending proof for register");
        ctx.send_proof_single("hydentity".into(), proof, blob_tx_hash.clone())
            .await?;

        info!("➡️  Waiting for height 2");
        ctx.wait_height(2).await?;

        info!("Hydentity: {:?}", executor.hydentity);

        info!("➡️  Sending blob to transfer 25 tokens from faucet to bob");

        let mut tx = ProvableBlobTx::new("faucet.hydentity".into());

        verify_identity(
            &mut tx,
            "hydentity".into(),
            &executor.hydentity,
            "password".to_string(),
        )?;

        transfer(&mut tx, "hyllar".into(), "bob.hydentity".to_string(), 25)?;

        ctx.send_provable_blob_tx(&tx).await?;

        let tx = executor.process(tx)?;
        let mut proofs = tx.iter_prove();

        let hydentity_proof = proofs.next().unwrap().0.await?;
        let hyllar_proof = proofs.next().unwrap().0.await?;

        info!("➡️  Sending proof for hydentity");
        ctx.send_proof_single("hydentity".into(), hydentity_proof, blob_tx_hash.clone())
            .await?;

        info!("➡️  Sending proof for hyllar");
        ctx.send_proof_single("hyllar".into(), hyllar_proof, blob_tx_hash)
            .await?;

        info!("➡️  Waiting for height 5");
        ctx.wait_height(5).await?;

        let contract = ctx.get_contract("hyllar").await?;
        let state: hyllar::HyllarToken = contract.state.try_into()?;
        let state = hyllar::HyllarTokenContract::init(state, "caller".into());
        assert_eq!(
            state
                .balance_of("bob.hydentity")
                .expect("bob identity not found"),
            25
        );
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn hyllar_single_node() -> Result<()> {
        let ctx = E2ECtx::new_single(500).await?;
        scenario_hyllar(ctx).await
    }

    #[test_log::test(tokio::test)]
    async fn hyllar_multi_nodes() -> Result<()> {
        let ctx = E2ECtx::new_multi(2, 500).await?;

        scenario_hyllar(ctx).await
    }
}
