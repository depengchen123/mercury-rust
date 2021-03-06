use std::net::{SocketAddr, ToSocketAddrs};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use log::*;
use structopt::StructOpt;

use did::vault::{HdProfileVault, ProfileVault};
use did::*;
use mercury_home_protocol::*;

#[derive(Debug, StructOpt)]
#[structopt(
    name = "mercury-home",
    about = "Mercury Home Node daemon",
    setting = structopt::clap::AppSettings::ColoredHelp
)]
struct CliConfig {
    #[structopt(long = "keyvault-dir", value_name = "DIR", parse(from_os_str))]
    /// Configuration directory to load keyvault from.
    /// Default: OS-specific app_cfg_dir/prometheus
    pub keyvault_dir: Option<PathBuf>,

    #[structopt(long = "profileid", value_name = "ID")]
    /// Key ID within keyvault to be used for authentication by this node.
    pub profile_id: Option<ProfileId>,

    #[structopt(
        long = "profile-backup",
        default_value = "/tmp/mercury/home/profile-backups",
        parse(from_os_str),
        value_name = "PATH"
    )]
    /// Directory path to store profile backups
    profile_backup_path: PathBuf,

    #[structopt(
        long = "host-relations",
        default_value = "/tmp/mercury/home/host-relations",
        parse(from_os_str),
        value_name = "PATH"
    )]
    /// Directory path to store hosted profiles in
    host_relations_path: PathBuf,

    #[structopt(
        long = "distributed-storage",
        default_value = "127.0.0.1:6161",
        value_name = "IP:PORT"
    )]
    /// Network address of public profile storage
    distributed_storage_address: String,

    #[structopt(long = "tcp", default_value = "0.0.0.0:2077", value_name = "IP:Port")]
    /// Listen on this socket to serve TCP clients
    socket_addr: String,
}

impl CliConfig {
    const CONFIG_PATH: &'static str = "home.cfg";

    pub fn new() -> Self {
        util::parse_config::<Self>(Self::CONFIG_PATH)
    }
}

pub struct Config {
    private_storage_path: PathBuf,
    host_relations_path: PathBuf,
    distributed_storage_address: SocketAddr,
    _vault: Arc<HdProfileVault>,
    signer: Rc<dyn Signer>,
    listen_socket: SocketAddr, // TODO consider using Vec if listening on several network devices is needed
}

impl Config {
    pub fn new() -> Self {
        let cli = CliConfig::new();

        let vault_path =
            did::paths::vault_path(cli.keyvault_dir).expect("Failed to get keyvault path");
        let vault = Arc::new(HdProfileVault::load(&vault_path).expect(&format!(
            "Profile vault is required but failed to load from {}",
            vault_path.to_string_lossy()
        )));

        let profile_id = cli.profile_id.or_else(|| vault.get_active().expect("Failed to get active profile") )
            .expect("Profile id is needed for authenticating the node, but neither command line argument is specified, nor active profile is set in vault");
        let signer = vault.clone().signer(&profile_id).unwrap();

        info!("homenode profile id: {}", signer.profile_id());
        info!("homenode public key: {}", signer.public_key());

        let listen_socket = cli
            .socket_addr
            .to_socket_addrs()
            .unwrap()
            .next()
            .expect("Failed to parse socket address for private storage");

        let distributed_storage_address = cli
            .distributed_storage_address
            .to_socket_addrs()
            .unwrap()
            .next()
            .expect("Failed to parse socket address for distributed storage");

        Self {
            private_storage_path: cli.profile_backup_path,
            host_relations_path: cli.host_relations_path,
            distributed_storage_address,
            _vault: vault,
            signer,
            listen_socket,
        }
    }

    pub fn profile_backup_path(&self) -> &PathBuf {
        &self.private_storage_path
    }
    pub fn host_relations_path(&self) -> &PathBuf {
        &self.host_relations_path
    }
    pub fn distributed_storage_address(&self) -> &SocketAddr {
        &self.distributed_storage_address
    }
    pub fn signer(&self) -> Rc<dyn Signer> {
        self.signer.clone()
    }
    pub fn listen_socket(&self) -> &SocketAddr {
        &self.listen_socket
    }
}
