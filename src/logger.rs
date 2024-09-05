use std::fmt::Display;

use anyhow::Context;
use tracing::{error, warn};
// A simple way to log without interrupting fluency
pub trait LogMe<T> {
    fn log_warn<C: Display + Send + Sync + 'static>(self, context_msg: C) -> anyhow::Result<T>;
    fn log_error<C: Display + Send + Sync + 'static>(self, context_msg: C) -> anyhow::Result<T>;
}

// Will log a warning in case of error
// WARN {context_msg}: {cause}
impl<T, Error: std::error::Error + Send + Sync + 'static> LogMe<T> for Result<T, Error> {
    fn log_warn<C: Display + Send + Sync + 'static>(self, context_msg: C) -> anyhow::Result<T> {
        let res = self.context(context_msg);
        if let Err(e) = &res {
            warn!("{:#}", e);
        }
        res
    }

    fn log_error<C: Display + Send + Sync + 'static>(self, context_msg: C) -> anyhow::Result<T> {
        let res = self.context(context_msg);
        if let Err(e) = &res {
            error!("{:#}", e);
        }
        res
    }
}
