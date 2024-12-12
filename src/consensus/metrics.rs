use opentelemetry::{
    metrics::{Counter, Gauge},
    InstrumentationScope, KeyValue,
};

pub struct ConsensusMetrics {
    signature_error: Counter<u64>,
    start_new_round: Counter<u64>,
    start_new_round_error: Counter<u64>,
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
    pub fn global(id: String) -> ConsensusMetrics {
        let scope = InstrumentationScope::builder(id).build();
        let my_meter = opentelemetry::global::meter_with_scope(scope);

        ConsensusMetrics {
            signature_error: my_meter.u64_counter("signature_error").build(),
            start_new_round: my_meter.u64_counter("start_new_round").build(),
            start_new_round_error: my_meter.u64_counter("start_new_round_error").build(),
            prepare: my_meter.u64_counter("prepare").build(),
            prepare_error: my_meter.u64_counter("prepare_error").build(),
            prepare_vote: my_meter.u64_counter("prepare_vote").build(),
            prepare_vote_error: my_meter.u64_counter("prepare_vote_error").build(),
            confirm: my_meter.u64_counter("confirm").build(),
            confirm_error: my_meter.u64_counter("confirm_error").build(),
            confirm_ack: my_meter.u64_counter("confirm_ack").build(),
            confirm_ack_error: my_meter.u64_counter("confirm_ack_error").build(),
            commit: my_meter.u64_counter("commit").build(),
            commit_error: my_meter.u64_counter("commit_error").build(),
            confirm_ack_commit_aggregate: my_meter
                .u64_counter("confirm_ack_commit_aggregate")
                .build(),
            confirmed_ack_gauge: my_meter.u64_gauge("confirmed_ack_gauge").build(),
            prepare_votes_gauge: my_meter.u64_gauge("prepare_votes_gauge").build(),
            prepare_votes_aggregation: my_meter.u64_counter("prepare_votes_aggregation").build(),
        }
    }

    pub fn signature_error(&self, kind: &'static str) {
        self.signature_error.add(1, &[KeyValue::new("kind", kind)]);
    }

    pub fn start_new_round(&self, kind: &'static str) {
        self.start_new_round.add(1, &[KeyValue::new("kind", kind)]);
    }
    pub fn start_new_round_error(&self, kind: &'static str) {
        self.start_new_round_error
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
