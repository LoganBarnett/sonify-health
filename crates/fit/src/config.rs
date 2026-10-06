use crate::command::Command;
use rust_template_foundation::MergeConfig;
use sonify_health_lib::{LogFormat, LogLevel};

// The daemon and the CLI share one `config.toml` and one `sonify_health_*`
// environment prefix, both keyed by the app name.  Naming the project here
// rather than this binary keeps the tool on that same configuration.
#[derive(Debug, Clone, MergeConfig)]
#[merge_config(app_name = "sonify-health")]
pub struct Config {
  #[merge_config(common)]
  pub log_level: LogLevel,
  #[merge_config(common)]
  pub log_format: LogFormat,

  #[merge_config(subcommand)]
  pub command: Command,
}
