use anyhow::Result;
use assertables::assert_ok;
use client_sdk::{BlobTransaction, Hashable, ProofData};
use hyle_contract_sdk::{Blob, BlobIndex, ContractName, Identity};
use sha3::Digest;
use tracing::info;

use crate::{
    bus::{bus_client, BusClientReceiver, BusClientSender},
    data_availability::DataEvent,
    mempool::api::RestApiMessage,
    model::data_availability::ShaBlob,
    utils::{crypto::BlstCrypto, integration_test::NodeIntegrationCtxBuilder},
};

use super::{
    data_availability::{BlstSignatureBlob, NativeProof},
    ProofTransaction,
};

bus_client! {
    struct Client {
        sender(RestApiMessage),
        receiver(DataEvent),
    }
}

#[test_log::test(tokio::test)]
async fn test_blst_native_verifier() {
    let contract_name: ContractName = "blst".into();
    let identity: Identity = format!("bob.{contract_name}").into();

    let crypto = BlstCrypto::new_random();

    let data = vec![1, 2, 3, 4, 5, 6];

    let signature = crypto
        .sign([data.clone(), identity.0.as_bytes().to_vec()].concat())
        .expect("failed to sign");

    let blob = BlstSignatureBlob {
        identity: identity.clone(),
        data,
        signature: signature.signature.signature.0,
        public_key: crypto.validator_pubkey().0.clone(),
    };

    let res = scenario("blst".into(), identity, blob.as_blob()).await;
    assert_ok!(res);
}

#[test_log::test(tokio::test)]
async fn test_sha3_256_native_verifier() {
    let contract_name: ContractName = "sha3_256".into();
    let identity: Identity = format!("bob.{contract_name}").into();

    let data = vec![1, 2, 3, 4, 5, 6];

    let mut hasher = sha3::Sha3_256::new();
    hasher.update(&data);
    let sha = hasher.finalize().to_vec();

    let blob = ShaBlob {
        identity: identity.clone(),
        data,
        sha,
    };

    let res = scenario(contract_name.clone(), identity, blob.as_blob(contract_name)).await;
    assert_ok!(res);
}

async fn scenario(contract_name: ContractName, identity: Identity, blob: Blob) -> Result<()> {
    let mut node_modules = NodeIntegrationCtxBuilder::new().await;
    node_modules.conf.consensus.slot_duration = 200;
    let mut node_modules = node_modules.build().await?;

    let mut node_client = Client::new_from_bus(node_modules.bus.new_handle()).await;

    // Wait until we process the genesis block
    node_modules.wait_for_processed_genesis().await?;

    let blob_tx = BlobTransaction {
        identity: identity.clone(),
        blobs: vec![blob.clone()],
    };
    let blob_tx_hash = blob_tx.hash();
    node_client.send(RestApiMessage::NewTx(blob_tx.clone().into()))?;

    let proof = NativeProof {
        tx_hash: blob_tx.hash(),
        index: BlobIndex(0),
        blobs: vec![blob],
    };
    let proof = bincode::encode_to_vec(proof, bincode::config::standard())?;

    node_client.send(RestApiMessage::NewTx(super::Transaction::from(
        ProofTransaction {
            contract_name: contract_name.clone(),
            proof: ProofData::Bytes(proof),
        },
    )))?;

    // Wait until we commit this TX
    loop {
        let evt: DataEvent = node_client.recv().await?;
        match evt {
            DataEvent::NewBlock(block) => {
                info!("Got Block");
                if block
                    .settled_blob_tx_hashes
                    .iter()
                    .any(|tx| tx == &blob_tx_hash)
                {
                    break;
                }
            }
        }
    }

    Ok(())
}
