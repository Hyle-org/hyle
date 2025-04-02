use sdk::hyle_model_utils::TimestampMs;

pub struct TimestampMsClock;

impl TimestampMsClock {
    #[cfg(not(feature = "turmoil"))]
    pub fn now() -> TimestampMs {
        TimestampMs(tokio::time::Instant::now().elapsed().as_millis())
    }

    #[cfg(feature = "turmoil")]
    pub fn now() -> TimestampMs {
        TimestampMs(turmoil::elapsed().as_millis())
    }
}
