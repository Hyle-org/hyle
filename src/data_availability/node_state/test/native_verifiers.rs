use anyhow::Result;
use assertables::assert_ok;
use client_sdk::{BlobTransaction, Hashable, ProofData};
use hyle_contract_sdk::{BlobIndex, ContractName, Identity};
use tracing::info;

use crate::{
    bus::{bus_client, BusClientReceiver, BusClientSender},
    data_availability::DataEvent,
    mempool::api::RestApiMessage,
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
async fn test_native_verifier() {
    assert_ok!(impl_test_native_verifier().await);
}

async fn impl_test_native_verifier() -> Result<()> {
    let mut node_modules = NodeIntegrationCtxBuilder::new().await;
    node_modules.conf.consensus.slot_duration = 200;
    let mut node_modules = node_modules.build().await?;

    let mut node_client = Client::new_from_bus(node_modules.bus.new_handle()).await;

    let contract_name = ContractName("blst".into());

    // Wait until we process the genesis block
    node_modules.wait_for_processed_genesis().await?;

    let crypto = BlstCrypto::new_random();

    let identity: Identity = "bob.blst".into();
    let data = vec![];

    let signature = crypto.sign([data.clone(), identity.0.as_bytes().to_vec()].concat())?;

    let blobs = vec![BlstSignatureBlob {
        identity: identity.clone(),
        data,
        signature: signature.signature.signature.0,
        public_key: crypto.validator_pubkey().0.clone(),
    }
    .as_blob()];

    let blob_tx = BlobTransaction {
        identity: identity.clone(),
        blobs: blobs.clone(),
    };
    let blob_tx_hash = blob_tx.hash();
    node_client.send(RestApiMessage::NewTx(blob_tx.clone().into()))?;

    let proof = NativeProof {
        tx_hash: blob_tx.hash(),
        index: BlobIndex(0),
        blobs,
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
