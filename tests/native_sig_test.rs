use fixtures::ctx::E2ECtx;
use tracing::info;

use hyle::model::ProofData;

mod fixtures;

use anyhow::Result;

mod e2e_native_sig {
    use hyle::{
        model::{
            data_availability::{BlstSignatureBlob, NativeProof},
            indexer::TransactionStatus,
        },
        utils::crypto::BlstCrypto,
    };
    use hyle_contract_sdk::{BlobIndex, Identity};

    use super::*;

    async fn scenario(ctx: E2ECtx) -> Result<()> {
        info!("➡️  Sending blob to register bob identity");

        let crypto = BlstCrypto::new_random();

        let identity: Identity = "bob.blst".into();
        let data = vec![];

        let signature = crypto.sign([data.clone(), identity.0.as_bytes().to_vec()].concat())?;

        let blobs = vec![BlstSignatureBlob {
            identity,
            data,
            signature: signature.signature.signature.0,
            public_key: crypto.validator_pubkey().0.clone(),
        }
        .as_blob()];

        let tx_hash = ctx.send_blob("bob.blst".into(), blobs.clone()).await?;

        let proof = NativeProof {
            tx_hash: tx_hash.clone(),
            index: BlobIndex(0),
            blobs,
        };
        let proof = bincode::encode_to_vec(proof, bincode::config::standard())?;

        info!("➡️  Sending proof for register");
        ctx.send_proof_single("blst".into(), ProofData::Bytes(proof), "".into())
            .await?;

        info!("➡️  Waiting for height 2");
        ctx.wait_height(2).await?;

        let tx = ctx
            .indexer_client()
            .get_transaction_by_hash(&tx_hash)
            .await?;

        assert_eq!(tx.transaction_status, TransactionStatus::Success);

        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn native_sig() -> Result<()> {
        let ctx = E2ECtx::new_multi_with_indexer(2, 500).await?;
        scenario(ctx).await
    }
}
