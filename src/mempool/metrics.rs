use opentelemetry::KeyValue;

pub struct MempoolMetrics {
    api_tx: opentelemetry::metrics::Counter<u64>,
    broadcasted_tx: opentelemetry::metrics::Counter<u64>,
    broadcasted_data_proposal: opentelemetry::metrics::Counter<u64>,
    broadcasted_data_proposal_only_for: opentelemetry::metrics::Counter<u64>,
    sent_data_vote: opentelemetry::metrics::Counter<u64>,
    sent_sync_request: opentelemetry::metrics::Counter<u64>,
    sent_sync_reply: opentelemetry::metrics::Counter<u64>,
    in_memory_tx: opentelemetry::metrics::Gauge<u64>,
    batches: opentelemetry::metrics::Counter<u64>,
}

impl MempoolMetrics {
    pub fn global(node_name: String) -> MempoolMetrics {
        let my_meter = opentelemetry::global::meter(node_name);

        MempoolMetrics {
            api_tx: my_meter.u64_counter("api_tx").init(),
            broadcasted_tx: my_meter.u64_counter("broadcasted_tx").init(),
            broadcasted_data_proposal: my_meter.u64_counter("broadcasted_data_proposal").init(),
            broadcasted_data_proposal_only_for: my_meter
                .u64_counter("broadcasted_data_proposal_only_for")
                .init(),
            sent_data_vote: my_meter.u64_counter("sent_data_vote").init(),
            sent_sync_request: my_meter.u64_counter("sent_sync_request").init(),
            sent_sync_reply: my_meter.u64_counter("sent_sync_reply").init(),
            in_memory_tx: my_meter.u64_gauge("in_memory_tx").init(),
            batches: my_meter.u64_counter("batches").init(),
        }
    }

    pub fn add_api_tx(&self, kind: String) {
        self.api_tx.add(1, &[KeyValue::new("kind", kind)]);
    }
    pub fn snapshot_pending_tx(&self, nb: usize) {
        self.in_memory_tx
            .record(nb as u64, &[KeyValue::new("status", "pending")])
    }
    pub fn snapshot_batched_tx(&self, nb: usize) {
        self.in_memory_tx
            .record(nb as u64, &[KeyValue::new("status", "batch")])
    }
    pub fn add_broadcasted_tx(&self, kind: String) {
        self.broadcasted_tx.add(1, &[KeyValue::new("kind", kind)])
    }
    pub fn add_batch(&self) {
        self.batches.add(1, &[])
    }
    pub fn add_broadcasted_data_proposal(&self, kind: String) {
        self.broadcasted_data_proposal
            .add(1, &[KeyValue::new("kind", kind)])
    }
    pub fn add_broadcasted_data_proposal_only_for(&self, kind: String) {
        self.broadcasted_data_proposal_only_for
            .add(1, &[KeyValue::new("kind", kind)])
    }
    pub fn add_sent_data_vote(&self, kind: String) {
        self.sent_data_vote.add(1, &[KeyValue::new("kind", kind)])
    }
    pub fn add_sent_sync_request(&self, kind: String) {
        self.sent_sync_request
            .add(1, &[KeyValue::new("kind", kind)])
    }
    pub fn add_sent_sync_reply(&self, kind: String) {
        self.sent_sync_reply.add(1, &[KeyValue::new("kind", kind)])
    }
}
