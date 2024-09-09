use std::collections::HashMap;

use anyhow::{bail, Error, Result};

use crate::model::{BlobTransaction, ContractName, ProofTransaction, Transaction, TxHash};
use model::{
    BlobsHash, Contract, Timeouts, UnsettledBlobDetail, UnsettledTransaction, VerificationStatus,
};

mod model;

#[derive(Default, Debug, Clone)]
pub struct NodeState {
    pub timeouts: Timeouts,
    pub contracts: HashMap<ContractName, Contract>,
    pub transactions: HashMap<TxHash, UnsettledTransaction>,
    pub tx_order: HashMap<ContractName, Vec<TxHash>>,
}

impl NodeState {
    pub fn handle_transaction(self, transaction: Transaction) -> Result<NodeState, Error> {
        let mut new_state = self.clone();

        let res = match transaction.transaction_data {
            crate::model::TransactionData::Blob(tx) => new_state.handle_blob_tx(tx),
            crate::model::TransactionData::Proof(tx) => new_state.handle_proof(tx),
            crate::model::TransactionData::RegisterContract(_) => todo!(),
        };

        res.map(|_| new_state)
    }

    pub fn handle_blob_tx(&mut self, _tx: BlobTransaction) -> Result<(), Error> {
        todo!()
    }

    pub fn handle_proof(&mut self, tx: ProofTransaction) -> Result<(), Error> {
        // Diverse verifications
        let unsettled_tx = match self.transactions.get(&tx.tx_hash) {
            Some(tx) => tx,
            None => bail!("Tx is either settled or does not exists."),
        };

        // Verify proof
        let blob_detail = Self::verify_proof(&tx)?;

        // extract publicInputs
        // hash payloads
        let blobs_hash = Self::hash_blobs(&blob_detail)?;

        // some verifications
        if blobs_hash != unsettled_tx.blobs_hash {
            todo!()
        }

        self.update_tx(&tx, blob_detail)?;

        // check if tx can be settled
        if self.is_settled(&tx.tx_hash) {
            // settle tx
            Self::settle_tx()?;
        }

        Ok(())
    }

    fn update_tx(
        &mut self,
        tx: &ProofTransaction,
        _blob_detail: UnsettledBlobDetail,
    ) -> Result<(), Error> {
        let _unsettled_tx = match self.transactions.get_mut(&tx.tx_hash) {
            Some(tx) => tx,
            None => bail!("Tx is either settled or does not exists."),
        };

        // unsettled_tx.blobs[tx.blob_index] = blob_detail;

        Ok(())
    }

    fn is_settled(&self, tx_hash: &TxHash) -> bool {
        let tx = match self.transactions.get(tx_hash) {
            Some(tx) => tx,
            None => {
                return false;
            }
        };

        let mut settled = true;
        for blob in &tx.blobs {
            settled = settled && (blob.verification_status == VerificationStatus::Success);
        }

        settled
    }

    fn verify_proof(_tx: &ProofTransaction) -> Result<UnsettledBlobDetail, Error> {
        todo!()
    }

    fn hash_blobs(_blob: &UnsettledBlobDetail) -> Result<BlobsHash, Error> {
        todo!()
    }

    fn settle_tx() -> Result<(), Error> {
        todo!()
    }
}
