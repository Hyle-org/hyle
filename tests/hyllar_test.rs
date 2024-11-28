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
        info!("➡️  Sending blob to register faucet identity");
        let blob_tx_hash = ctx
            .send_blob(
                "faucet.hydentity".into(),
                vec![(
                    ContractName("hydentity".to_owned()),
                    IdentityAction::RegisterIdentity {
                        account: "faucet.hydentity".to_string(),
                    },
                )
                    .into()],
            )
            .await?;

        let proof = load_encoded_receipt_from_file("./tests/proofs/register.hydentity.risc0.proof");

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

        info!("➡️  Sending blob to transfer 10 tokens from faucet to bob");
        let blob_tx_hash = ctx
            .send_blob(
                "faucet.hydentity".into(),
                vec![
                    (
                        ContractName("hydentity".to_owned()),
                        IdentityAction::VerifyIdentity {
                            account: "faucet.hydentity".to_string(),
                            nonce: 0,
                            blobs_hash: vec!["".into()],
                        },
                    )
                        .into(),
                    (
                        ContractName("hyllar".to_owned()),
                        ERC20Action::Transfer {
                            recipient: "bob.hydentity".to_string(),
                            amount: 100,
                        },
                    )
                        .into(),
                ],
            )
            .await?;

        let hydentity_proof =
            load_encoded_receipt_from_file("./tests/proofs/transfer.hydentity.risc0.proof");
        let hyllar_proof =
            load_encoded_receipt_from_file("./tests/proofs/transfer.hyllar.risc0.proof");

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
            100
        );
        assert_eq!(
            state
                .balance_of("faucet.hydentity")
                .expect("faucet identity not found"),
            900
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
