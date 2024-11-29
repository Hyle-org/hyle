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

mod e2e_amm {
    use amm::AmmAction;
    use fixtures::contracts::AmmContract;
    use hydentity::AccountInfo;
    use hyle_contract_sdk::{
        erc20::{ERC20Action, ERC20},
        identity_provider::{IdentityAction, IdentityVerification},
        BlobIndex, ContractName,
    };

    use super::*;

    async fn scenario_amm(ctx: E2ECtx) -> Result<()> {
        // Here is the flow that we are going to test:
        // Register bob in hydentity

        // Send 25 hyllar from faucet to bob
        // Send 50 hyllar2 from faucet to bob

        // Register new amm contract "amm2"

        // Bob approves 100 hyllar to amm2
        // Bob approves 100 hyllar2 to amm2

        // Bob registers a new pair in amm
        //    By sending 20 hyllar to amm
        //    By sending 50 hyllar2 to amm

        // Bob swaps 5 hyllar for 10 hyllar2
        //    By sending 5 hyllar to amm
        //    By sending 10 hyllar2 to bob (from amm)

        let contract_hyllar = ctx.get_contract("hyllar").await?;
        let state =
            hyllar::HyllarTokenContract::init(contract_hyllar.state.try_into()?, "caller".into());
        let hyllar_initial_total_amount: u128 = state
            .balance_of("faucet.hydentity")
            .expect("faucet identity not found");

        let contract_hyllar2 = ctx.get_contract("hyllar2").await?;
        let state =
            hyllar::HyllarTokenContract::init(contract_hyllar2.state.try_into()?, "caller".into());
        let hyllar2_initial_total_amount: u128 = state
            .balance_of("faucet.hydentity")
            .expect("faucet identity not found");

        ///////////////////// bob identity registration /////////////////////
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

        let contract_hydentity = ctx.get_contract("hydentity").await?;
        let state: hydentity::Hydentity = contract_hydentity.state.try_into()?;

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
        /////////////////////////////////////////////////////////////////////

        ///////////////// sending hyllar from faucet to bob /////////////////
        info!("➡️  Sending blob to transfer 25 hyllar from faucet to bob");
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
        let bob_transfer_proof = load_encoded_receipt_from_file(
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
            ProofData::Bytes(bob_transfer_proof),
            blob_tx_hash,
        )
        .await?;

        info!("➡️  Waiting for height 5");
        ctx.wait_height(5).await?;

        let contract_hyllar = ctx.get_contract("hyllar").await?;
        let state: hyllar::HyllarToken = contract_hyllar.state.try_into()?;
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
            hyllar_initial_total_amount - 25
        );
        /////////////////////////////////////////////////////////////////////

        ///////////////// sending hyllar2 from faucet to bob /////////////////
        info!("➡️  Sending blob to transfer 50 hyllar2 from faucet to bob");
        let blob_tx_hash = ctx
            .send_blob(
                "faucet.hydentity".into(),
                vec![
                    IdentityAction::VerifyIdentity {
                        account: "faucet.hydentity".to_string(),
                        nonce: 1,
                    }
                    .as_blob(ContractName("hydentity".to_owned())),
                    ERC20Action::Transfer {
                        recipient: "bob.hydentity".to_string(),
                        amount: 50,
                    }
                    .as_blob(ContractName("hyllar2".to_owned()), None, None),
                ],
            )
            .await?;

        let hydentity_proof = load_encoded_receipt_from_file(
            "./tests/proofs/transfer.50-hyllar2-to-bob.hydentity.risc0.proof",
        );
        let bob_transfer_proof = load_encoded_receipt_from_file(
            "./tests/proofs/transfer.50-hyllar2-to-bob.hyllar2.risc0.proof",
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
            "hyllar2".into(),
            ProofData::Bytes(bob_transfer_proof),
            blob_tx_hash,
        )
        .await?;

        info!("➡️  Waiting for height 5");
        ctx.wait_height(5).await?;

        let contract_hyllar2 = ctx.get_contract("hyllar2").await?;
        let state: hyllar::HyllarToken = contract_hyllar2.state.try_into()?;
        let state = hyllar::HyllarTokenContract::init(state, "caller".into());
        assert_eq!(
            state
                .balance_of("bob.hydentity")
                .expect("bob identity not found"),
            50
        );
        assert_eq!(
            state
                .balance_of("faucet.hydentity")
                .expect("faucet identity not found"),
            hyllar2_initial_total_amount - 50
        );
        /////////////////////////////////////////////////////////////////////

        ///////////////////// amm contract registration /////////////////////
        info!("➡️  Registring amm contract");
        const AMM_CONTRACT_NAME: &str = "amm2";
        ctx.register_contract::<AmmContract>(AMM_CONTRACT_NAME)
            .await?;
        /////////////////////////////////////////////////////////////////////

        //////////////////// Bob approves AMM on hyllar /////////////////////
        info!("➡️  Sending blob to approve amm on hyllar");
        let blob_tx_hash = ctx
            .send_blob(
                "bob.hydentity".into(),
                vec![
                    IdentityAction::VerifyIdentity {
                        account: "bob.hydentity".to_string(),
                        nonce: 0,
                    }
                    .as_blob(ContractName("hydentity".to_owned())),
                    ERC20Action::Approve {
                        spender: AMM_CONTRACT_NAME.to_string(),
                        amount: 100,
                    }
                    .as_blob(ContractName("hyllar".to_owned()), None, None),
                ],
            )
            .await?;

        let hydentity_proof = load_encoded_receipt_from_file(
            "./tests/proofs/approve.100-hyllar.hydentity.risc0.proof",
        );
        let bob_approve_hyllar_proof =
            load_encoded_receipt_from_file("./tests/proofs/approve.100-hyllar.hyllar.risc0.proof");

        info!("➡️  Sending proof for hydentity");
        ctx.send_proof(
            "hydentity".into(),
            ProofData::Bytes(hydentity_proof),
            blob_tx_hash.clone(),
        )
        .await?;

        info!("➡️  Sending proof for approve hyllar");
        ctx.send_proof(
            "hyllar".into(),
            ProofData::Bytes(bob_approve_hyllar_proof),
            blob_tx_hash.clone(),
        )
        .await?;

        info!("➡️  Waiting for height 5");
        ctx.wait_height(5).await?;

        let contract_hyllar = ctx.get_contract("hyllar").await?;
        let state: hyllar::HyllarToken = contract_hyllar.state.try_into()?;
        let state = hyllar::HyllarTokenContract::init(state, "caller".into());
        assert_eq!(
            state
                .allowance("bob.hydentity", "amm2")
                .expect("bob identity not found"),
            100
        );
        /////////////////////////////////////////////////////////////////////

        //////////////////// Bob approves AMM on hyllar2 /////////////////////
        info!("➡️  Sending blob to approve amm on hyllar2");
        let blob_tx_hash = ctx
            .send_blob(
                "bob.hydentity".into(),
                vec![
                    IdentityAction::VerifyIdentity {
                        account: "bob.hydentity".to_string(),
                        nonce: 1,
                    }
                    .as_blob(ContractName("hydentity".to_owned())),
                    ERC20Action::Approve {
                        spender: AMM_CONTRACT_NAME.to_string(),
                        amount: 100,
                    }
                    .as_blob(ContractName("hyllar2".to_owned()), None, None),
                ],
            )
            .await?;

        let hydentity_proof = load_encoded_receipt_from_file(
            "./tests/proofs/approve.100-hyllar2.hydentity.risc0.proof",
        );
        let bob_approve_hyllar2_proof = load_encoded_receipt_from_file(
            "./tests/proofs/approve.100-hyllar2.hyllar2.risc0.proof",
        );

        info!("➡️  Sending proof for hydentity");
        ctx.send_proof(
            "hydentity".into(),
            ProofData::Bytes(hydentity_proof),
            blob_tx_hash.clone(),
        )
        .await?;

        info!("➡️  Sending proof for approve hyllar2");
        ctx.send_proof(
            "hyllar2".into(),
            ProofData::Bytes(bob_approve_hyllar2_proof),
            blob_tx_hash.clone(),
        )
        .await?;

        info!("➡️  Waiting for height 5");
        ctx.wait_height(5).await?;

        let contract_hyllar2 = ctx.get_contract("hyllar2").await?;
        let state: hyllar::HyllarToken = contract_hyllar2.state.try_into()?;
        let state = hyllar::HyllarTokenContract::init(state, "caller".into());
        assert_eq!(
            state
                .allowance("bob.hydentity", "amm2")
                .expect("bob identity not found"),
            100
        );
        /////////////////////////////////////////////////////////////////////

        /////////////// Creating new pair hyllar/hyllar2 on amm ///////////////
        info!("➡️  Sending blob to transfer 50 hyllar2 from faucet to bob");
        let blob_tx_hash = ctx
            .send_blob(
                "bob.hydentity".into(),
                vec![
                    IdentityAction::VerifyIdentity {
                        account: "bob.hydentity".to_string(),
                        nonce: 2,
                    }
                    .as_blob(ContractName("hydentity".to_owned())),
                    AmmAction::NewPair {
                        pair: ("hyllar".to_string(), "hyllar2".to_string()),
                        amounts: (20, 50),
                    }
                    .as_blob(
                        ContractName(AMM_CONTRACT_NAME.to_owned()),
                        None,
                        Some(vec![BlobIndex(2), BlobIndex(3)]),
                    ),
                    ERC20Action::TransferFrom {
                        sender: "bob.hydentity".to_string(),
                        recipient: AMM_CONTRACT_NAME.to_string(),
                        amount: 20,
                    }
                    .as_blob(
                        ContractName("hyllar".to_owned()),
                        Some(BlobIndex(1)),
                        None,
                    ),
                    ERC20Action::TransferFrom {
                        sender: "bob.hydentity".to_string(),
                        recipient: AMM_CONTRACT_NAME.to_string(),
                        amount: 50,
                    }
                    .as_blob(
                        ContractName("hyllar2".to_owned()),
                        Some(BlobIndex(1)),
                        None,
                    ),
                ],
            )
            .await?;

        let hydentity_proof = load_encoded_receipt_from_file(
            "./tests/proofs/new-pair-hyllar-hyllar2.hydentity.risc0.proof",
        );
        let bob_new_pair_proof = load_encoded_receipt_from_file(
            "./tests/proofs/new-pair-hyllar-hyllar2.amm.risc0.proof",
        );
        let bob_transfer_hyllar_proof = load_encoded_receipt_from_file(
            "./tests/proofs/transfer.20-hyllar-to-amm.hyllar.risc0.proof",
        );
        let bob_transfer_hyllar2_proof = load_encoded_receipt_from_file(
            "./tests/proofs/transfer.50-hyllar2-to-amm.hyllar2.risc0.proof",
        );

        info!("➡️  Sending proof for hydentity");
        ctx.send_proof(
            "hydentity".into(),
            ProofData::Bytes(hydentity_proof),
            blob_tx_hash.clone(),
        )
        .await?;

        info!("➡️  Sending proof for new pair");
        ctx.send_proof(
            AMM_CONTRACT_NAME.into(),
            ProofData::Bytes(bob_new_pair_proof),
            blob_tx_hash.clone(),
        )
        .await?;

        info!("➡️  Sending proof for hyllar");
        ctx.send_proof(
            "hyllar".into(),
            ProofData::Bytes(bob_transfer_hyllar_proof),
            blob_tx_hash.clone(),
        )
        .await?;

        info!("➡️  Sending proof for hyllar2");
        ctx.send_proof(
            "hyllar2".into(),
            ProofData::Bytes(bob_transfer_hyllar2_proof),
            blob_tx_hash,
        )
        .await?;

        info!("➡️  Waiting for height 5");
        ctx.wait_height(5).await?;

        let contract_hyllar = ctx.get_contract("hyllar").await?;
        let state: hyllar::HyllarToken = contract_hyllar.state.try_into()?;
        let state = hyllar::HyllarTokenContract::init(state, "caller".into());
        assert_eq!(
            state
                .balance_of("bob.hydentity")
                .expect("bob identity not found"),
            5
        );
        assert_eq!(
            state.balance_of("amm2").expect("amm2 identity not found"),
            20
        );
        assert_eq!(
            state
                .balance_of("faucet.hydentity")
                .expect("faucet identity not found"),
            hyllar_initial_total_amount - 25
        );

        let contract_hyllar2 = ctx.get_contract("hyllar2").await?;
        let state: hyllar::HyllarToken = contract_hyllar2.state.try_into()?;
        let state = hyllar::HyllarTokenContract::init(state, "caller".into());
        assert_eq!(
            state
                .balance_of("bob.hydentity")
                .expect("bob identity not found"),
            0
        );
        assert_eq!(
            state.balance_of("amm2").expect("amm2 identity not found"),
            50
        );
        assert_eq!(
            state
                .balance_of("faucet.hydentity")
                .expect("faucet identity not found"),
            hyllar2_initial_total_amount - 50
        );
        //////////////////////////////////////////////////////////////////////

        /////////////////////// Bob actually swaps //////////////////////////
        info!("➡️  Sending blob for bob to swap 5 hyllar for 10 hyllar2");
        let blob_tx_hash = ctx
            .send_blob(
                "bob.hydentity".into(),
                vec![
                    IdentityAction::VerifyIdentity {
                        account: "bob.hydentity".to_string(),
                        nonce: 3,
                    }
                    .as_blob(ContractName("hydentity".to_owned())),
                    AmmAction::Swap {
                        pair: ("hyllar".to_string(), "hyllar2".to_string()),
                    }
                    .as_blob(
                        ContractName(AMM_CONTRACT_NAME.to_owned()),
                        None,
                        Some(vec![BlobIndex(2), BlobIndex(3)]),
                    ),
                    ERC20Action::Transfer {
                        recipient: "bob.hydentity".to_string(),
                        amount: 5,
                    }
                    .as_blob(
                        ContractName("hyllar".to_owned()),
                        Some(BlobIndex(1)),
                        None,
                    ),
                    ERC20Action::TransferFrom {
                        sender: "bob.hydentity".to_string(),
                        recipient: AMM_CONTRACT_NAME.to_string(),
                        amount: 10,
                    }
                    .as_blob(
                        ContractName("hyllar2".to_owned()),
                        Some(BlobIndex(1)),
                        None,
                    ),
                ],
            )
            .await?;

        let hydentity_proof =
            load_encoded_receipt_from_file("./tests/proofs/swap.hydentity.risc0.proof");
        let bob_swap_proof =
            load_encoded_receipt_from_file("./tests/proofs/swap.hyllar.hyllar2.risc0.proof");
        let bob_transfer_proof = load_encoded_receipt_from_file(
            "./tests/proofs/transfer.5-hyllar-to-amm.hyllar.risc0.proof",
        );
        let amm_transfer_from_proof = load_encoded_receipt_from_file(
            "./tests/proofs/transfer.10-hyllar2-to-bob.hyllar2.risc0.proof",
        );

        info!("➡️  Sending proof for hydentity");
        ctx.send_proof(
            "hydentity".into(),
            ProofData::Bytes(hydentity_proof),
            blob_tx_hash.clone(),
        )
        .await?;

        info!("➡️  Sending swap for amm");
        ctx.send_proof(
            AMM_CONTRACT_NAME.into(),
            ProofData::Bytes(bob_swap_proof),
            blob_tx_hash.clone(),
        )
        .await?;

        info!("➡️  Sending transfer for hyllar");
        ctx.send_proof(
            "hyllar".into(),
            ProofData::Bytes(bob_transfer_proof),
            blob_tx_hash.clone(),
        )
        .await?;

        info!("➡️  Sending swap for hyllar2");
        ctx.send_proof(
            "hyllar2".into(),
            ProofData::Bytes(amm_transfer_from_proof),
            blob_tx_hash.clone(),
        )
        .await?;

        info!("➡️  Waiting for height 5");
        ctx.wait_height(5).await?;

        let contract_hyllar = ctx.get_contract("hyllar").await?;
        let state: hyllar::HyllarToken = contract_hyllar.state.try_into()?;
        let state = hyllar::HyllarTokenContract::init(state, "caller".into());
        assert_eq!(
            state
                .balance_of("bob.hydentity")
                .expect("bob identity not found"),
            5
        );
        assert_eq!(
            state.balance_of("amm2").expect("amm identity not found"),
            20
        );
        assert_eq!(
            state
                .balance_of("faucet.hydentity")
                .expect("faucet identity not found"),
            hyllar_initial_total_amount - 25
        );

        let contract_hyllar2 = ctx.get_contract("hyllar2").await?;
        let state: hyllar::HyllarToken = contract_hyllar2.state.try_into()?;
        let state = hyllar::HyllarTokenContract::init(state, "caller".into());
        assert_eq!(
            state
                .balance_of("bob.hydentity")
                .expect("bob identity not found"),
            10
        );
        assert_eq!(
            state.balance_of("amm2").expect("amm identity not found"),
            40
        );
        assert_eq!(
            state
                .balance_of("faucet.hydentity")
                .expect("faucet identity not found"),
            hyllar2_initial_total_amount - 50
        );
        /////////////////////////////////////////////////////////////////////
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn amm_single_node() -> Result<()> {
        let ctx = E2ECtx::new_single(500).await?;
        scenario_amm(ctx).await
    }

    #[test_log::test(tokio::test)]
    async fn amm_multi_nodes() -> Result<()> {
        let ctx = E2ECtx::new_multi(2, 500).await?;

        scenario_amm(ctx).await
    }
}
