use std::time::{SystemTime, UNIX_EPOCH};

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

pub fn get_current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_secs()
}

pub fn get_current_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_millis() as u64
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, BorshDeserialize, BorshSerialize)]
pub struct TimestampMs(pub u64);

impl TimestampMs {
    pub fn now() -> TimestampMs {
        TimestampMs(get_current_timestamp_ms())
    }
}
