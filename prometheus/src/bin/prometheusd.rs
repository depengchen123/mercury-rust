use failure::Fallible;
use log::*;
use structopt::StructOpt;

use prometheus::{init_logger, Daemon, Options};

fn main() {
    match run() {
        Ok(()) => {}
        Err(e) => error!("Failed with error: {}", e),
    }
}

fn run() -> Fallible<()> {
    let options = Options::from_args();

    init_logger(&options)?;

    let daemon = Daemon::start(options)?;

    // let registry = prometheus::ClaimSchemaRegistry::import_folder(&std::path::PathBuf::from("./schemas"))?;
    // for (_k, v) in registry.schemas {
    //     info!("***\n{:#?}\n***", v);
    // }

    // NOTE HTTP server already handles signals internally unless the no_signals option is set.
    match daemon.join() {
        Err(e) => info!("Daemon thread failed with error: {:?}", e),
        Ok(_) => info!("Graceful shut down"),
    };

    Ok(())
}
