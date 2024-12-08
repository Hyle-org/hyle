use fixtures::ctx::E2ECtx;
use std::{fs::File, io::Read};
use tracing::info;

use hyle::model::ProofData;

mod fixtures;

use anyhow::Result;

pub fn load_encoded_receipt_from_file(path: &str) -> Vec<u8> {
    let mut file = File::open(path).expect("Failed to open proof file");
    let mut encoded_receipt = Vec::new();
    file.read_to_end(&mut encoded_receipt)
        .expect("Failed to read file content");
    encoded_receipt
}

mod e2e_hyllar {
    use hydentity::AccountInfo;
    use hyle_contract_sdk::{
        erc20::{ERC20Action, ERC20},
        identity_provider::{IdentityAction, IdentityVerification},
        ContractName,
    };

    use super::*;

    async fn scenario_hyllar(ctx: E2ECtx) -> Result<()> {
        info!("➡️  Sending blob to register bob identity");
        let blob_tx_hash = ctx
            .send_blob(
                "bob.hydentity".into(),
                vec![IdentityAction::RegisterIdentity {
                    account: "bob.hydentity".to_string(),
                }
                .as_blob(ContractName("hydentity".to_owned()))],
            )
            .await?;

        let proof =
            load_encoded_receipt_from_file("./tests/proofs/register.bob.hydentity.risc0.proof");

        info!("➡️  Sending proof for register");
        ctx.send_proof(
            "hydentity".into(),
            ProofData::Bytes(proof),
            blob_tx_hash.clone(),
        )
        .await?;

        info!("➡️  Waiting for height 2");
        ctx.wait_height(2).await?;

        let contract = ctx.get_contract("hydentity").await?;
        let state: hydentity::Hydentity = contract.state.try_into()?;

        let expected_info = serde_json::to_string(&AccountInfo {
            hash: "b6baa13a27c933bb9f7df812108407efdff1ec3c3ef8d803e20eed7d4177d596".to_string(),
            nonce: 0,
        });
        assert_eq!(
            state
                .get_identity_info("faucet.hydentity")
                .expect("faucet identity not found"),
            expected_info.unwrap() // hash for "faucet.hydentity::password"
        );

        info!("➡️  Sending blob to transfer 25 tokens from faucet to bob");
        let blob_tx_hash = ctx
            .send_blob(
                "faucet.hydentity".into(),
                vec![
                    IdentityAction::VerifyIdentity {
                        account: "faucet.hydentity".to_string(),
                        nonce: 0,
                    }
                    .as_blob(ContractName("hydentity".to_owned())),
                    ERC20Action::Transfer {
                        recipient: "bob.hydentity".to_string(),
                        amount: 25,
                    }
                    .as_blob(ContractName("hyllar".to_owned()), None, None),
                ],
            )
            .await?;

        let hydentity_proof = load_encoded_receipt_from_file(
            "./tests/proofs/transfer.25-hyllar-to-bob.hydentity.risc0.proof",
        );
        let hyllar_proof = load_encoded_receipt_from_file(
            "./tests/proofs/transfer.25-hyllar-to-bob.hyllar.risc0.proof",
        );

        info!("➡️  Sending proof for hydentity");
        ctx.send_proof(
            "hydentity".into(),
            ProofData::Bytes(hydentity_proof),
            blob_tx_hash.clone(),
        )
        .await?;

        info!("➡️  Sending proof for hyllar");
        ctx.send_proof(
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
        assert_eq!(
            state
                .balance_of("faucet.hydentity")
                .expect("faucet identity not found"),
            98_999_999_975
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
