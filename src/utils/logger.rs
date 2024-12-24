use anyhow::Result;
use std::fmt::Display;
use tracing::{error, warn};
use tracing::{level_filters::LevelFilter, Subscriber};
use tracing_subscriber::{
    fmt::{format, FormatEvent, FormatFields},
    prelude::*,
    registry::LookupSpan,
    EnvFilter,
};

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

/// Custom formatter that appends node_name in front of full logs
struct NodeNameFormatter<T> {
    node_name: String,
    base_formatter: T,
}

impl<S, N, T> FormatEvent<S, N> for NodeNameFormatter<T>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
    T: FormatEvent<S, N>,
{
    fn format_event(
        &self,
        ctx: &tracing_subscriber::fmt::FmtContext<'_, S, N>,
        mut writer: format::Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> std::fmt::Result {
        write!(&mut writer, "{} ", &self.node_name,)?;
        self.base_formatter.format_event(ctx, writer, event)
    }
}

pub enum TracingMode {
    /// Default tracing, for running a node locally
    Full,
    /// JSON tracing, for running a node in a container
    Json,
    /// Full tracing + node name, for e2e tests
    NodeName,
}

/// Setup tracing - stdout subscriber
/// stdout defaults to INFO & sled to INFO even if RUST_LOG is set to e.g. debug (unless it contains "sled")
pub fn setup_tracing(mode: TracingMode, node_name: String) -> Result<()> {
    let mut filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env()?;

    let var = std::env::var("RUST_LOG").unwrap_or("".to_string());
    if !var.contains("sled") {
        filter = filter.add_directive("sled=info".parse()?);
    }
    if !var.contains("risc0_zkvm") {
        filter = filter.add_directive("risc0_zkvm=info".parse()?);
    }
    if !var.contains("tower_http") {
        // API request/response debug tracing
        filter = filter.add_directive("tower_http::trace=debug".parse()?);
    }

    // Can't use match inline because these are different return types
    match mode {
        TracingMode::Full => register_global_subscriber(filter, tracing_subscriber::fmt::layer()),
        TracingMode::Json => register_global_subscriber(
            filter,
            tracing_subscriber::fmt::layer().event_format(tracing_subscriber::fmt::format().json()),
        ),
        TracingMode::NodeName => register_global_subscriber(
            filter,
            tracing_subscriber::fmt::layer().event_format(NodeNameFormatter {
                node_name,
                base_formatter: tracing_subscriber::fmt::format(),
            }),
        ),
    };

    Ok(())
}

fn register_global_subscriber<T, S>(filter: EnvFilter, fmt_layer: T)
where
    S: Subscriber,
    T: tracing_subscriber::Layer<S> + Send + Sync,
    tracing_subscriber::filter::Filtered<T, tracing_subscriber::EnvFilter, S>:
        tracing_subscriber::Layer<tracing_subscriber::Registry>,
{
    tracing_subscriber::registry()
        .with(fmt_layer.with_filter(filter))
        .init();
}
