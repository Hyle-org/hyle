use borsh::{BorshDeserialize, BorshSerialize};
use client_sdk::transaction_builder::TxExecutorHandler;
use sdk::{BlobTransaction, Calldata, ContractName, Hashed, TxContext, TxHash};
use std::{collections::VecDeque, fmt::Debug, vec};

#[derive(Default, BorshSerialize, BorshDeserialize)]
pub struct TxExecutor<Contract> {
    unsettled_txs: VecDeque<(BlobTransaction, TxContext)>,
    // unsettled_txs: VecDeque<(TxHash, Vec<(Blob, Calldata)>, TxContext)>,
    state_history: VecDeque<(TxHash, Contract)>,
    contract_name: ContractName,
    contract: Contract,
}

impl<Contract> TxExecutor<Contract>
where
    Contract: TxExecutorHandler + Debug + Clone + Send + Sync + 'static,
{
    /// This function executes the blob transaction and returns the outputs of the contract.
    /// It also keeps track of the transaction as unsettled and the state history.
    pub fn execute_tx(
        &mut self,
        blob_tx: &BlobTransaction,
        tx_ctx: Option<TxContext>,
    ) -> anyhow::Result<Vec<Vec<u8>>> {
        let initial_state = self.contract.clone();

        let mut hyle_outputs = vec![];

        for (blob_index, blob) in blob_tx.blobs.iter().enumerate() {
            // Filter out blobs that are not for the orderbook contract
            if blob.contract_name == self.contract_name {
                let calldata = Calldata {
                    identity: blob_tx.identity.clone(),
                    tx_hash: blob_tx.hashed(),
                    private_input: vec![],
                    blobs: blob_tx.blobs.clone().into(),
                    index: blob_index.into(),
                    tx_ctx: tx_ctx.clone(),
                    tx_blob_count: blob_tx.blobs.len(),
                };
                // Execute the blob
                match self.contract.handle(&calldata) {
                    Err(e) => {
                        // Transaction is invalid, we need to revert the state
                        self.contract = initial_state;
                        anyhow::bail!("Error while executing contract: {e}")
                    }
                    Ok(hyle_output) => {
                        hyle_outputs.push(hyle_output.program_outputs);
                    }
                }
            }
        }
        // Blobs execution went fine.
        // We keep track of the transaction as unsettled
        self.unsettled_txs
            .push_back((blob_tx.clone(), tx_ctx.clone().unwrap()));
        // We also keep track of the state history
        self.state_history
            .push_back((blob_tx.hashed(), self.contract.clone()));

        Ok(hyle_outputs)
    }

    /// This function is called when the transaction is confirmed as failed.
    /// It reverts the state and reexecutes all unsettled transaction after this one.
    pub fn cancel_tx(&mut self, tx_hash: &TxHash) -> anyhow::Result<()> {
        let tx_pos = self
            .unsettled_txs
            .iter()
            .position(|(blob_tx, _)| blob_tx.hashed() == *tx_hash)
            .ok_or(anyhow::anyhow!(
                "Transaction not found in unsettled transactions"
            ))?;
        self.unsettled_txs.remove(tx_pos);

        // Revert the contract state to the state before the cancelled transaction
        if let Some((_, state)) = self.state_history.get(tx_pos) {
            self.contract = state.clone();
            // Remove all state history after and including tx_pos
            self.state_history.truncate(tx_pos);
        } else {
            anyhow::bail!("State history not found for the cancelled transaction");
        }

        // Re-execute all unsettled transactions after the cancelled one
        let reexecute_txs: Vec<(BlobTransaction, TxContext)> =
            self.unsettled_txs.drain(tx_pos..).collect();
        for (blob_tx, tx_ctx) in reexecute_txs.iter() {
            // Ignore outputs, just update state and unsettled_txs/state_history
            let _ = self.execute_tx(blob_tx, Some(tx_ctx.clone()))?;
        }
        Ok(())
    }
}
