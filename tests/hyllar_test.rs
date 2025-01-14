#![allow(clippy::unwrap_used, clippy::expect_used)]
use fixtures::ctx::E2ECtx;
use fixtures::proofs::HyrunProofGen;
use tracing::info;

use hyle::model::ProofData;

mod fixtures;

use anyhow::Result;

mod e2e_hyllar {
    use hydentity::AccountInfo;
    use hyle_contract_sdk::{
        erc20::{ERC20Action, ERC20},
        identity_provider::{IdentityAction, IdentityVerification},
        ContractAction, ContractName,
    };
    use hyrun::CliCommand;

    use super::*;

    async fn scenario_hyllar(ctx: E2ECtx) -> Result<()> {
        let proof_generator = HyrunProofGen::setup_working_directory();

        info!("➡️  Sending blob to register bob identity");
        let blob_tx_hash = ctx
            .send_blob(
                "bob.hydentity".into(),
                vec![IdentityAction::RegisterIdentity {
                    account: "bob.hydentity".to_string(),
                }
                .as_blob(ContractName::new("hydentity"))],
            )
            .await?;

        proof_generator
            .generate_proof(
                &ctx,
                CliCommand::Hydentity {
                    command: hyrun::HydentityArgs::Register {
                        account: "bob.hydentity".to_owned(),
                    },
                },
                "bob.hydentity",
                "password",
                None,
            )
            .await;

        let proof = proof_generator.read_proof(0);

        info!("➡️  Sending proof for register");
        ctx.send_proof_single(
            "hydentity".into(),
            ProofData::Bytes(proof),
            blob_tx_hash.clone(),
        )
        .await?;

        info!("➡️  Waiting for height 2");
        ctx.wait_height(2).await?;

        let contract = ctx.get_contract("hydentity").await?;
        let state: hydentity::Hydentity = contract.state.try_into()?;

        // faucet_start_nonce = 0 in single-mode, N in multi-node(N) mode
        let faucet_start_nonce = serde_json::from_str::<AccountInfo>(
            state
                .get_identity_info("faucet.hydentity")
                .expect("faucet identity not found")
                .as_str(),
        )
        .expect("Failed to parse faucet identity info")
        .nonce;

        info!("➡️  Sending blob to transfer 25 tokens from faucet to bob");
        let blob_tx_hash = ctx
            .send_blob(
                "faucet.hydentity".into(),
                vec![
                    IdentityAction::VerifyIdentity {
                        account: "faucet.hydentity".to_string(),
                        nonce: faucet_start_nonce,
                    }
                    .as_blob(ContractName::new("hydentity")),
                    ERC20Action::Transfer {
                        recipient: "bob.hydentity".to_string(),
                        amount: 25,
                    }
                    .as_blob(ContractName::new("hyllar"), None, None),
                ],
            )
            .await?;

        proof_generator
            .generate_proof(
                &ctx,
                CliCommand::Hyllar {
                    command: hyrun::HyllarArgs::Transfer {
                        recipient: "bob.hydentity".to_owned(),
                        amount: 25,
                    },
                    hyllar_contract_name: "hyllar".to_owned(),
                },
                "faucet.hydentity",
                "password",
                Some(faucet_start_nonce),
            )
            .await;

        let hydentity_proof = proof_generator.read_proof(0);
        let hyllar_proof = proof_generator.read_proof(1);

        info!("➡️  Sending proof for hydentity");
        ctx.send_proof_single(
            "hydentity".into(),
            ProofData::Bytes(hydentity_proof),
            blob_tx_hash.clone(),
        )
        .await?;

        info!("➡️  Sending proof for hyllar");
        ctx.send_proof_single(
            "hyllar".into(),
            ProofData::Bytes(hyllar_proof),
            blob_tx_hash,
        )
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
