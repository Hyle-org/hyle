use anyhow::Result;
use std::fmt::Display;
// use tracing::{error, warn};
use tracing::{level_filters::LevelFilter, Subscriber};
use tracing_subscriber::{
    fmt::{format, FormatEvent, FormatFields},
    prelude::*,
    registry::LookupSpan,
    EnvFilter,
};

// moved trait to macro. current impl will only point to the module parent. i.e., `consensus_tests` instead of `consensus_tests::e2e_consensus`
// if you want to get the precise module, call the macro within every function that'll use `.log_warn` or `.log_error`
// if you don't like these hoops, consider adjusting the api to fit a little by taking in the module_path directly into the `.log` function
#[macro_export]
macro_rules! log_me_impl {
  ($t: ty) => {
  // Will log a warning in case of error
  // WARN {context_msg}: {cause}
  trait LogMe<T> {
     fn log_warn<C: std::fmt::Display + Send + Sync + 'static>(self, context_msg: C) -> anyhow::Result<T>;
     fn log_error<C: std::fmt::Display + Send + Sync + 'static>(self, context_msg: C) -> anyhow::Result<T>;
  }
  impl<T, Error: Into<anyhow::Error> + std::fmt::Display + Send + Sync + 'static> LogMe<T>
    for $t {
      fn log_warn<C: std::fmt::Display + Send + Sync + 'static>(self, context_msg: C) -> anyhow::Result<T> {
          match self {
              Err(e) => {
                  let ae: anyhow::Error = e.into();
                  let ae = ae.context(context_msg);

                  tracing::error!(target: module_path!(), "{:#}", ae);

                  Err(ae)
              }
              Ok(t) => Ok(t),
          }
      }

      fn log_error<C: std::fmt::Display + Send + Sync + 'static>(self, context_msg: C) -> anyhow::Result<T> {
          match self {
              Err(e) => {
                  let ae: anyhow::Error = e.into();
                  let ae = ae.context(context_msg);

                  tracing::error!(target: module_path!(), "{:#}", ae);

                  Err(ae)
              }
              Ok(t) => Ok(t),
          }
      }
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
/// stdout defaults to INFO to INFO even if RUST_LOG is set to e.g. debug
pub fn setup_tracing(mode: TracingMode, node_name: String) -> Result<()> {
    let mut filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env()?;

    let var = std::env::var("RUST_LOG").unwrap_or("".to_string());
    if !var.contains("risc0_zkvm") {
        filter = filter.add_directive("risc0_zkvm=info".parse()?);
    }
    if !var.contains("tokio") {
        filter = filter.add_directive("tokio=info".parse()?);
        filter = filter.add_directive("runtime=info".parse()?);
    }
    if !var.contains("fjall") {
        filter = filter.add_directive("fjall=warn".parse()?);
    }
    if !var.contains("opentelemetry") {
        filter = filter.add_directive("opentelemetry=warn".parse()?);
        filter = filter.add_directive("opentelemetry_sdk=warn".parse()?);
    }
    if !var.contains("risc0_zkvm") {
        std::env::set_var(
            "RUST_LOG",
            format!("{var},risc0_zkvm=warn,risc0_circuit_rv32im=warn,risc0_binfmt=warn"),
        );
        filter = filter.add_directive("risc0_zkvm=warn".parse()?);
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
