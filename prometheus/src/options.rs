use std::net::SocketAddr;
use std::path::PathBuf;

use structopt::StructOpt;

#[derive(Debug, StructOpt)]
#[structopt(
    name = "prometheusd",
    about = "Prometheus service daemon",
    setting = structopt::clap::AppSettings::ColoredHelp
)]
pub struct Options {
    #[structopt(long = "listen", default_value = "127.0.0.1:8080", value_name = "IP:PORT")]
    /// IPv4/6 address to listen on serving REST requests.
    pub listen_on: SocketAddr,

    #[structopt(long, value_name = "DIR", parse(from_os_str))]
    /// Configuration directory to pick vault and profile info from.
    /// Default: OS-specific app_cfg_dir/prometheus
    pub config_dir: Option<PathBuf>,

    #[structopt(long, value_name = "DIR", parse(from_os_str))]
    /// Directory that contains all claim schema definitions.
    /// Default: OS-specific app_cfg_dir/prometheus/schemas
    pub schemas_dir: Option<PathBuf>,

    #[structopt(long = "repository", default_value = "127.0.0.1:6161", value_name = "IP:PORT")]
    /// IPv4/6 address of the remote profile repository.
    pub remote_repo_address: SocketAddr,

    #[structopt(long = "timeout", default_value = "10", value_name = "SECS")]
    /// Number of seconds used for network timeouts
    pub network_timeout_secs: u64,

    #[structopt(long, default_value = "log4rs.yml", value_name = "FILE", parse(from_os_str))]
    /// Config file for log4rs (YAML).
    pub logger_config: PathBuf,
}
