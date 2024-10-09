use opentelemetry::{
    metrics::{Counter, Gauge},
    KeyValue,
};

use crate::validator_registry::ValidatorId;

pub struct ConsensusMetrics {
    signature_error: Counter<u64>,
    start_new_slot: Counter<u64>,
    start_new_slot_error: Counter<u64>,
    prepare: Counter<u64>,
    prepare_error: Counter<u64>,
    prepare_vote: Counter<u64>,
    prepare_vote_error: Counter<u64>,
    confirm: Counter<u64>,
    confirm_error: Counter<u64>,
    confirm_ack: Counter<u64>,
    confirm_ack_error: Counter<u64>,
    commit: Counter<u64>,
    commit_error: Counter<u64>,
    confirm_ack_commit_aggregate: Counter<u64>,
    confirmed_ack_gauge: Gauge<u64>,
    prepare_votes_gauge: Gauge<u64>,
    prepare_votes_aggregation: Counter<u64>,
}

impl ConsensusMetrics {
    pub fn global(id: ValidatorId) -> ConsensusMetrics {
        let my_meter = opentelemetry::global::meter(id.0.clone());

        ConsensusMetrics {
            signature_error: my_meter.u64_counter("signature_error").init(),
            start_new_slot: my_meter.u64_counter("start_new_slot").init(),
            start_new_slot_error: my_meter.u64_counter("start_new_slot_error").init(),
            prepare: my_meter.u64_counter("prepare").init(),
            prepare_error: my_meter.u64_counter("prepare_error").init(),
            prepare_vote: my_meter.u64_counter("prepare_vote").init(),
            prepare_vote_error: my_meter.u64_counter("prepare_vote_error").init(),
            confirm: my_meter.u64_counter("confirm").init(),
            confirm_error: my_meter.u64_counter("confirm_error").init(),
            confirm_ack: my_meter.u64_counter("confirm_ack").init(),
            confirm_ack_error: my_meter.u64_counter("confirm_ack_error").init(),
            commit: my_meter.u64_counter("commit").init(),
            commit_error: my_meter.u64_counter("commit_error").init(),
            confirm_ack_commit_aggregate: my_meter
                .u64_counter("confirm_ack_commit_aggregate")
                .init(),
            confirmed_ack_gauge: my_meter.u64_gauge("confirmed_ack_gauge").init(),
            prepare_votes_gauge: my_meter.u64_gauge("prepare_votes_gauge").init(),
            prepare_votes_aggregation: my_meter.u64_counter("prepare_votes_aggregation").init(),
        }
    }

    pub fn signature_error(&self, kind: &'static str) {
        self.signature_error.add(1, &[KeyValue::new("kind", kind)]);
    }

    pub fn start_new_slot(&self, kind: &'static str) {
        self.start_new_slot.add(1, &[KeyValue::new("kind", kind)]);
    }
    pub fn start_new_slot_error(&self, kind: &'static str) {
        self.start_new_slot_error
            .add(1, &[KeyValue::new("kind", kind)]);
    }
    pub fn prepare(&self) {
        self.prepare.add(1, &[]);
    }
    pub fn prepare_error(&self, kind: &'static str) {
        self.prepare_error.add(1, &[KeyValue::new("kind", kind)]);
    }
    pub fn prepare_vote(&self, kind: &'static str) {
        self.prepare_vote.add(1, &[KeyValue::new("kind", kind)]);
    }
    pub fn prepare_vote_error(&self, kind: &'static str) {
        self.prepare_vote_error
            .add(1, &[KeyValue::new("kind", kind)]);
    }

    pub fn prepare_votes_aggregation(&self) {
        self.prepare_votes_aggregation.add(1, &[]);
    }

    pub fn confirm(&self, kind: &'static str) {
        self.confirm.add(1, &[KeyValue::new("kind", kind)]);
    }
    pub fn confirm_error(&self, kind: &'static str) {
        self.confirm_error.add(1, &[KeyValue::new("kind", kind)]);
    }
    pub fn confirm_ack(&self, kind: &'static str) {
        self.confirm_ack.add(1, &[KeyValue::new("kind", kind)]);
    }
    pub fn confirm_ack_error(&self, kind: &'static str) {
        self.confirm_ack_error
            .add(1, &[KeyValue::new("kind", kind)]);
    }

    pub fn confirm_ack_commit_aggregate(&self) {
        self.confirm_ack_commit_aggregate.add(1, &[]);
    }

    pub fn confirmed_ack_gauge(&self, nb: u64) {
        self.confirmed_ack_gauge.record(nb, &[])
    }
    pub fn prepare_votes_gauge(&self, nb: u64) {
        self.prepare_votes_gauge.record(nb, &[])
    }

    pub fn commit(&self) {
        self.commit.add(1, &[]);
    }
    pub fn commit_error(&self, kind: &'static str) {
        self.commit_error.add(1, &[KeyValue::new("kind", kind)]);
    }
}
