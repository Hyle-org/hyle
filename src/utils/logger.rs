use std::fmt::Display;

use tracing::{error, warn};
// A simple way to log without interrupting fluency
pub trait LogMe<T> {
    fn log_warn<C: Display + Send + Sync + 'static>(self, context_msg: C) -> anyhow::Result<T>;
    fn log_error<C: Display + Send + Sync + 'static>(self, context_msg: C) -> anyhow::Result<T>;
}

// Will log a warning in case of error
// WARN {context_msg}: {cause}
impl<T, Error: Into<anyhow::Error> + Display + Send + Sync + 'static> LogMe<T>
    for Result<T, Error>
{
    fn log_warn<C: Display + Send + Sync + 'static>(self, context_msg: C) -> anyhow::Result<T> {
        match self {
            Err(e) => {
                let ae: anyhow::Error = e.into();
                let ae = ae.context(context_msg);
                warn!("{:#}", ae);
                Err(ae)
            }
            Ok(t) => Ok(t),
        }
    }

    fn log_error<C: Display + Send + Sync + 'static>(self, context_msg: C) -> anyhow::Result<T> {
        match self {
            Err(e) => {
                let ae: anyhow::Error = e.into();
                let ae = ae.context(context_msg);
                error!("{:#}", ae);
                Err(ae)
            }
            Ok(t) => Ok(t),
        }
    }
}
