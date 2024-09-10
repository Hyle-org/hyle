use std::collections::HashMap;

use anyhow::{bail, Error, Result};
use ordered_tx_map::OrderedTxMap;

use crate::model::{
    BlobTransaction, BlobsHash, Block, BlockHeight, ContractName, Hashable, ProofTransaction,
    RegisterContractTransaction, Transaction, TxHash,
};
use model::{Contract, Timeouts, UnsettledBlobDetail, UnsettledTransaction, VerificationStatus};

mod model;
mod ordered_tx_map;

#[derive(Default, Debug, Clone)]
pub struct NodeState {
    timeouts: Timeouts,
    current_height: BlockHeight,
    contracts: HashMap<ContractName, Contract>,
    unsettled_transactions: OrderedTxMap,
}

impl NodeState {
    pub fn handle_new_block(&self, block: Block) -> NodeState {
        let mut new_state = self.clone();

        new_state.clear_timeouts(&block.height);
        new_state.current_height = block.height;
        new_state
    }

    pub fn handle_transaction(&self, transaction: Transaction) -> Result<NodeState, Error> {
        let mut new_state = self.clone();

        let res = match transaction.transaction_data {
            crate::model::TransactionData::Blob(tx) => new_state.handle_blob_tx(tx),
            crate::model::TransactionData::Proof(tx) => new_state.handle_proof(tx),
            crate::model::TransactionData::RegisterContract(tx) => {
                new_state.handle_register_contract(tx)
            }
        };

        res.map(|_| new_state)
    }

    fn handle_register_contract(&mut self, tx: RegisterContractTransaction) -> Result<(), Error> {
        self.contracts.insert(
            tx.contract_name.clone(),
            Contract {
                name: tx.contract_name,
                program_id: tx.program_id,
                state: tx.state_digest,
            },
        );

        Ok(())
    }

    fn handle_blob_tx(&mut self, tx: BlobTransaction) -> Result<(), Error> {
        let (tx_hash, blobs_hash) = hash_transaction(&tx);

        let blobs: Vec<UnsettledBlobDetail> = tx
            .blobs
            .iter()
            .map(|blob| UnsettledBlobDetail {
                contract_name: blob.contract_name.clone(),
                verification_status: VerificationStatus::WaitingProof,
                hyle_output: None,
            })
            .collect();

        self.unsettled_transactions.add(UnsettledTransaction {
            identity: tx.identity,
            hash: tx_hash.clone(),
            blobs_hash,
            blobs,
        });

        // Update timeouts
        self.timeouts.set(tx_hash, self.current_height);

        Ok(())
    }

    fn handle_proof(&mut self, tx: ProofTransaction) -> Result<(), Error> {
        // Diverse verifications
        let unsettled_tx = match self.unsettled_transactions.get(&tx.tx_hash) {
            Some(tx) => tx,
            None => bail!("Tx is either settled or does not exists."),
        };

        if !self
            .unsettled_transactions
            .is_next_unsettled_tx(&tx.tx_hash, &tx.contract_name)
        {
            // TODO: buffer this ProofTransaction to be handled later
            bail!("Another tx needs to be settled before.");
        }

        // Verify proof
        let blob_detail = Self::verify_proof(&tx)?;

        // hash payloads
        let blobs_hash = Self::extract_blobs_hash(&blob_detail)?;

        // some verifications
        if blobs_hash != unsettled_tx.blobs_hash {
            bail!("Proof blobs hash do not correspond to transaction blobs hash.")
        }

        self.update_state_tx(&tx, blob_detail)?;

        // check if tx can be settled
        let is_next_to_settle = self
            .unsettled_transactions
            .is_next_unsettled_tx(&tx.tx_hash, &tx.contract_name);

        if is_next_to_settle && self.is_settled(&tx.tx_hash) {
            // settle tx
            self.settle_tx(&tx)?;
        }

        Ok(())
    }

    fn clear_timeouts(&mut self, height: &BlockHeight) {
        let dropped = self.timeouts.drop(height);
        for tx in dropped {
            self.unsettled_transactions.remove(&tx);
        }
    }

    fn update_state_tx(
        &mut self,
        tx: &ProofTransaction,
        blob_detail: UnsettledBlobDetail,
    ) -> Result<(), Error> {
        let unsettled_tx = match self.unsettled_transactions.get_mut(&tx.tx_hash) {
            Some(tx) => tx,
            None => bail!("Tx is either settled or does not exists."),
        };

        // TODO: better not using "as usize"
        unsettled_tx.blobs[tx.blob_index.0 as usize] = blob_detail;

        Ok(())
    }

    fn is_settled(&self, tx_hash: &TxHash) -> bool {
        let tx = match self.unsettled_transactions.get(tx_hash) {
            Some(tx) => tx,
            None => {
                return false;
            }
        };

        tx.blobs
            .iter()
            .all(|blob| blob.verification_status == VerificationStatus::Success)
    }

    fn verify_proof(tx: &ProofTransaction) -> Result<UnsettledBlobDetail, Error> {
        // TODO real implementation
        Ok(UnsettledBlobDetail {
            contract_name: tx.contract_name.clone(),
            verification_status: VerificationStatus::Success,
            hyle_output: None,
        })
    }

    fn extract_blobs_hash(_blob: &UnsettledBlobDetail) -> Result<BlobsHash, Error> {
        todo!()
    }

    fn settle_tx(&mut self, tx: &ProofTransaction) -> Result<(), Error> {
        let unsettled_tx = match self.unsettled_transactions.get_mut(&tx.tx_hash) {
            Some(tx) => tx,
            None => bail!("Tx to settle not found!"),
        };

        if let Some(contract) = self.contracts.get_mut(&tx.contract_name) {
            if let Some(blob_detail) = unsettled_tx
                .blobs
                .iter()
                .find(|b| b.contract_name == tx.contract_name)
            {
                if let Some(hyle_output) = &blob_detail.hyle_output {
                    // Update contract state
                    contract.state = hyle_output.next_state.clone();
                } else {
                    bail!(
                        "Blob detail output not found for contract {} on transaction to settle :{}",
                        tx.contract_name,
                        tx.tx_hash
                    );
                }
            } else {
                bail!(
                    "Blob not found for contract {} on transaction to settle: {}",
                    tx.contract_name,
                    tx.tx_hash
                );
            }
        } else {
            bail!(
                "Contract {} not found when settling transaction {}",
                tx.contract_name,
                tx.tx_hash
            );
        }
        Ok(())
    }
}

// TODO: move it somewhere else ?
fn hash_transaction(tx: &BlobTransaction) -> (TxHash, BlobsHash) {
    (tx.hash(), tx.blobs_hash())
}
