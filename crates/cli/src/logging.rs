use sonify_health_lib::{LogFormat, LogLevel};
use tracing_subscriber::{
  fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer,
};

/// Initialise the global tracing subscriber.
///
/// On Unix, a journald layer is attempted first.  If the socket is
/// unavailable (e.g. macOS, containers without systemd), the layer
/// silently falls back to stderr so local development works unchanged.
pub fn init_logging(level: LogLevel, format: LogFormat) {
  let env_filter = EnvFilter::try_from_default_env()
    .unwrap_or_else(|_| EnvFilter::new(level.to_string()));

  let registry = tracing_subscriber::registry();

  #[cfg(unix)]
  let registry = {
    use tracing_subscriber::Layer as _;
    match tracing_journald::layer() {
      Ok(layer) => registry.with(Some(
        layer.with_filter(
          EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new(level.to_string())),
        ),
      )),
      Err(_) => registry.with(
        None::<
          tracing_subscriber::filter::Filtered<
            tracing_journald::Layer,
            EnvFilter,
            _,
          >,
        >,
      ),
    }
  };

  match format {
    LogFormat::Text => {
      registry
        .with(
          fmt::layer()
            .with_writer(std::io::stderr)
            .with_target(true)
            .with_line_number(true)
            .with_filter(env_filter),
        )
        .init();
    }
    LogFormat::Json => {
      registry
        .with(
          fmt::layer()
            .json()
            .with_writer(std::io::stderr)
            .with_target(true)
            .with_line_number(true)
            .with_filter(env_filter),
        )
        .init();
    }
  }
}
