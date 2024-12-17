use opentelemetry::{
    metrics::{Counter, Gauge},
    InstrumentationScope, KeyValue,
};

use crate::model::ValidatorPublicKey;

use super::{storage::DataProposal, QueryNewCut};

pub struct MempoolMetrics {
    signature_error: Counter<u64>,
    api_tx: Counter<u64>,
    data_proposal: Counter<u64>,
    proposed_txs: Counter<u64>,
    data_vote: Counter<u64>,
    sync_request: Counter<u64>,
    sync_reply: Counter<u64>,
    pending_tx: Gauge<u64>,
    new_cut: Counter<u64>,
}

impl MempoolMetrics {
    pub fn global(node_name: String) -> MempoolMetrics {
        let scope = InstrumentationScope::builder(node_name).build();
        let my_meter = opentelemetry::global::meter_with_scope(scope);

        let mempool = "mempool";

        MempoolMetrics {
            signature_error: my_meter
                .u64_counter(format!("{mempool}_signature_error"))
                .build(),
            api_tx: my_meter.u64_counter(format!("{mempool}_api_tx")).build(),
            data_proposal: my_meter
                .u64_counter(format!("{mempool}_data_proposal"))
                .build(),
            proposed_txs: my_meter
                .u64_counter(format!("{mempool}_proposed_txs"))
                .build(),
            data_vote: my_meter.u64_counter(format!("{mempool}_data_vote")).build(),
            sync_request: my_meter
                .u64_counter(format!("{mempool}_sync_request"))
                .build(),
            sync_reply: my_meter
                .u64_counter(format!("{mempool}_sync_reply"))
                .build(),
            pending_tx: my_meter.u64_gauge(format!("{mempool}_pending_tx")).build(),
            new_cut: my_meter.u64_counter(format!("{mempool}_new_cut")).build(),
        }
    }

    pub fn signature_error(&self, kind: &'static str) {
        self.signature_error.add(1, &[KeyValue::new("kind", kind)]);
    }

    pub fn add_api_tx(&self, kind: &'static str) {
        self.api_tx.add(1, &[KeyValue::new("tx_kind", kind)]);
    }
    pub fn snapshot_pending_tx(&self, nb: usize) {
        self.pending_tx
            .record(nb as u64, &[KeyValue::new("status", "pending")])
    }
    pub fn add_new_cut(&self, nc: &QueryNewCut) {
        self.new_cut.add(
            1,
            &[KeyValue::new(
                "nb_validators",
                nc.0.bonded().len().to_string(),
            )],
        )
    }

    pub fn add_proposed_txs(&self, dp: &DataProposal) {
        for tx in dp.txs.iter() {
            let tx_type: &'static str = (&tx.transaction_data).into();
            self.proposed_txs.add(1, &[KeyValue::new("kind", tx_type)]);
        }
    }

    pub fn add_data_proposal(&self, dp: &DataProposal) {
        let tx_nb = dp.txs.len();
        self.data_proposal
            .add(1, &[KeyValue::new("nb_txs", tx_nb.to_string())])
    }
    pub fn add_proposal_vote(&self, sender: &ValidatorPublicKey, dest: &ValidatorPublicKey) {
        self.data_vote.add(
            1,
            &[
                KeyValue::new("sender", format!("{}", sender)),
                KeyValue::new("dest", format!("{}", dest)),
            ],
        )
    }
    pub fn add_sync_request(&self, sender: &ValidatorPublicKey, dest: &ValidatorPublicKey) {
        self.sync_request.add(
            1,
            &[
                KeyValue::new("sender", format!("{}", sender)),
                KeyValue::new("dest", format!("{}", dest)),
            ],
        );
    }
    pub fn add_sync_reply(
        &self,
        sender: &ValidatorPublicKey,
        dest: &ValidatorPublicKey,
        lane_entries: usize,
    ) {
        self.sync_reply.add(
            1,
            &[
                KeyValue::new("nb_lane_entries", lane_entries.to_string()),
                KeyValue::new("sender", format!("{}", sender)),
                KeyValue::new("dest", format!("{}", dest)),
            ],
        )
    }
}
