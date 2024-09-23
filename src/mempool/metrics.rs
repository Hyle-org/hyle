use opentelemetry::KeyValue;

use crate::utils::conf::Conf;

pub struct MempoolMetrics {
    api_tx: opentelemetry::metrics::Counter<u64>,
    broadcasted_tx: opentelemetry::metrics::Counter<u64>,
    in_memory_tx: opentelemetry::metrics::Gauge<u64>,
    batches: opentelemetry::metrics::Counter<u64>,
}

impl MempoolMetrics {
    pub fn global(conf: &Conf) -> MempoolMetrics {
        let my_meter = opentelemetry::global::meter(conf.id.to_string().clone());

        MempoolMetrics {
            api_tx: my_meter.u64_counter("api_tx").init(),
            broadcasted_tx: my_meter.u64_counter("broadcasted_tx").init(),
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
}
