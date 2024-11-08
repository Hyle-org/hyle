use fixtures::{contracts::HyllarContract, ctx::E2ECtx};
use std::{fs::File, io::Read};
use tracing::info;

use hyle::model::{Blob, BlobReference, ProofData};

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
    use fixtures::contracts::HydentityContract;
    use hyle_contract_sdk::{
        erc20::{ERC20Action, ERC20},
        identity_provider::{IdentityAction, IdentityVerification},
    };

    use super::*;

    async fn scenario_hyllar(ctx: E2ECtx) -> Result<()> {
        info!("➡️  Registering contract hyllar");
        ctx.register_contract::<HydentityContract>("hydentity")
            .await?;
        ctx.register_contract::<HyllarContract>("hyllar").await?;

        info!("➡️  Sending blob to register faucet identity");
        let blob_tx_hash = ctx
            .send_blob(vec![Blob {
                contract_name: "hydentity".into(),
                data: IdentityAction::RegisterIdentity {
                    account: "faucet".to_string(),
                }
                .into(),
            }])
            .await?;

        let proof = load_encoded_receipt_from_file("./tests/proofs/register.hydentity.risc0.proof");

        info!("➡️  Sending proof for register");
        ctx.send_proof(
            vec![BlobReference {
                contract_name: "hydentity".into(),
                blob_tx_hash: blob_tx_hash.clone(),
                blob_index: hyle_contract_sdk::BlobIndex(0),
            }],
            ProofData::Bytes(proof),
        )
        .await?;

        info!("➡️  Waiting for height 2");
        ctx.wait_height(2).await?;

        let contract = ctx.get_contract("hydentity").await?;
        let state: hydentity::Hydentity = contract.state.try_into()?;
        assert_eq!(
            state
                .get_identity_info("faucet")
                .expect("faucet identity not found"),
            "6bcb9eabc039af650f4ea33f65e262649b838a3eb254207e033b4cf18cf01ba1" // hash for "faucet::password"
        );

        info!("➡️  Sending blob to transfer 10 tokens from faucet to bob");
        let blob_tx_hash = ctx
            .send_blob(vec![
                Blob {
                    contract_name: "hydentity".into(),
                    data: IdentityAction::VerifyIdentity {
                        account: "faucet".to_string(),
                        blobs_hash: vec!["".into()],
                    }
                    .into(),
                },
                Blob {
                    contract_name: "hyllar".into(),
                    data: ERC20Action::Transfer {
                        recipient: "bob".to_string(),
                        amount: 10,
                    }
                    .into(),
                },
            ])
            .await?;

        let hydentity_proof =
            load_encoded_receipt_from_file("./tests/proofs/transfer.hydentity.risc0.proof");
        let hyllar_proof =
            load_encoded_receipt_from_file("./tests/proofs/transfer.hyllar.risc0.proof");

        info!("➡️  Sending proof for hydentity");
        ctx.send_proof(
            vec![BlobReference {
                contract_name: "hydentity".into(),
                blob_tx_hash: blob_tx_hash.clone(),
                blob_index: hyle_contract_sdk::BlobIndex(0),
            }],
            ProofData::Bytes(hydentity_proof),
        )
        .await?;

        info!("➡️  Sending proof for hyllar");
        ctx.send_proof(
            vec![BlobReference {
                contract_name: "hyllar".into(),
                blob_tx_hash: blob_tx_hash.clone(),
                blob_index: hyle_contract_sdk::BlobIndex(1),
            }],
            ProofData::Bytes(hyllar_proof),
        )
        .await?;

        info!("➡️  Waiting for height 5");
        ctx.wait_height(5).await?;

        let contract = ctx.get_contract("hyllar").await?;
        let state: hyllar::HyllarToken = contract.state.try_into()?;
        let state = hyllar::HyllarTokenContract::init(state, "caller".into());
        assert_eq!(
            state.balance_of("bob").expect("faucet identity not found"),
            10
        );
        assert_eq!(
            state
                .balance_of("faucet")
                .expect("faucet identity not found"),
            990
        );
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn risc0_single_node() -> Result<()> {
        let ctx = E2ECtx::new_single(500).await?;
        scenario_hyllar(ctx).await
    }

    #[test_log::test(tokio::test)]
    async fn risc0_multi_nodes() -> Result<()> {
        let ctx = E2ECtx::new_multi(2, 500).await?;

        scenario_hyllar(ctx).await
    }
}
