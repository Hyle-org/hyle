use opentelemetry::{
    metrics::{Counter, Gauge},
    InstrumentationScope,
};

pub struct ConsensusMetrics {
    current_slot: Gauge<u64>,
    current_view: Gauge<u64>,

    last_started_round: Gauge<u64>,

    commit: Counter<u64>,

    pub on_prepare_ok: Counter<u64>,
    pub on_prepare_err: Counter<u64>,
    pub on_prepare_vote_ok: Counter<u64>,
    pub on_prepare_vote_err: Counter<u64>,
    pub on_confirm_ok: Counter<u64>,
    pub on_confirm_err: Counter<u64>,
    pub on_confirm_ack_ok: Counter<u64>,
    pub on_confirm_ack_err: Counter<u64>,
    pub on_timeout_ok: Counter<u64>,
    pub on_timeout_err: Counter<u64>,
    pub on_timeout_certificate_ok: Counter<u64>,
    pub on_timeout_certificate_err: Counter<u64>,
    pub on_sync_request_ok: Counter<u64>,
    pub on_sync_request_err: Counter<u64>,
    pub on_sync_reply_ok: Counter<u64>,
    pub on_sync_reply_err: Counter<u64>,
}

impl ConsensusMetrics {
    pub fn global(id: String) -> ConsensusMetrics {
        let scope = InstrumentationScope::builder(id).build();
        let my_meter = opentelemetry::global::meter_with_scope(scope);

        ConsensusMetrics {
            current_slot: my_meter.u64_gauge("current_slot").build(),
            current_view: my_meter.u64_gauge("current_view").build(),
            last_started_round: my_meter.u64_gauge("last_started_round").build(),
            commit: my_meter.u64_counter("commit").build(),
            on_prepare_ok: my_meter.u64_counter("on_prepare_ok").build(),
            on_prepare_err: my_meter.u64_counter("on_prepare_err").build(),
            on_prepare_vote_ok: my_meter.u64_counter("on_prepare_vote_ok").build(),
            on_prepare_vote_err: my_meter.u64_counter("on_prepare_vote_err").build(),
            on_confirm_ok: my_meter.u64_counter("on_confirm_ok").build(),
            on_confirm_err: my_meter.u64_counter("on_confirm_err").build(),
            on_confirm_ack_ok: my_meter.u64_counter("on_confirm_ack_ok").build(),
            on_confirm_ack_err: my_meter.u64_counter("on_confirm_ack_err").build(),
            on_timeout_ok: my_meter.u64_counter("on_timeout_ok").build(),
            on_timeout_err: my_meter.u64_counter("on_timeout_err").build(),
            on_timeout_certificate_ok: my_meter.u64_counter("on_timeout_certificate_ok").build(),
            on_timeout_certificate_err: my_meter.u64_counter("on_timeout_certificate_err").build(),
            on_sync_request_ok: my_meter.u64_counter("on_sync_request_ok").build(),
            on_sync_request_err: my_meter.u64_counter("on_sync_request_err").build(),
            on_sync_reply_ok: my_meter.u64_counter("on_sync_reply_ok").build(),
            on_sync_reply_err: my_meter.u64_counter("on_sync_reply_err").build(),
        }
    }

    // Stuff I'm keeping
    pub fn commit(&self) {
        self.commit.add(1, &[]);
    }
    pub fn at_round(&self, slot: u64, view: u64) {
        self.current_slot.record(slot, &[]);
        self.current_view.record(view, &[]);
    }

    pub fn start_new_round(&self, slot: u64) {
        self.last_started_round.record(slot, &[]);
    }
}
