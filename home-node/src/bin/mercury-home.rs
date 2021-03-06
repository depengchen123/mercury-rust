use std::cell::RefCell;
use std::rc::Rc;

use futures::{Future, Stream};
use log::*;
use tokio::net::tcp::TcpListener;
use tokio_current_thread as reactor;

use claims::repo::{DistributedPublicProfileRepository, FileProfileRepository};
use mercury_home_node::{config::*, server::*};
use mercury_home_protocol::{
    crypto::*, handshake, mercury_capnp::server_dispatcher::HomeDispatcherCapnProto, *,
};
use mercury_storage::asynch::fs::FileStore;
use mercury_storage::asynch::KeyAdapter;

fn main() {
    log4rs::init_file("log4rs.yml", Default::default()).unwrap();
    let config = Config::new();

    let signer = config.signer();
    let validator = Rc::new(CompositeValidator::default());

    let mut reactor = reactor::CurrentThread::new();

    let local_storage =
        Rc::new(RefCell::new(FileProfileRepository::new(config.profile_backup_path()).unwrap()));

    // TODO make file path configurable, remove rpc_storage address config parameter
    // TODO use some kind of real distributed storage here on the long run
    let mut distributed_storage =
        FileProfileRepository::new(&std::path::PathBuf::from("/tmp/cuccos")).unwrap();
    let avail_prof_res = reactor.block_on(distributed_storage.get_public(&signer.profile_id()));
    if avail_prof_res.is_err() {
        info!("Home node profile is not found on distributed public storage, saving node profile");
        use multiaddr::ToMultiaddr;
        let home_multiaddr = config.listen_socket().to_multiaddr().unwrap();
        let home_attrs = HomeFacet::new(vec![home_multiaddr], vec![]).to_attribute_map();
        let home_profile = Profile::new(signer.public_key(), 1, vec![], home_attrs);
        reactor.block_on(distributed_storage.set_public(home_profile)).unwrap();
    } else {
        info!("Home node profile is already available on distributed public storage");
    }

    let host_db = Rc::new(RefCell::new(KeyAdapter::new(
        FileStore::new(config.host_relations_path()).unwrap(),
    )));
    let distributed_storage = Rc::new(RefCell::new(distributed_storage));
    let server = Rc::new(HomeServer::new(validator, distributed_storage, local_storage, host_db));

    info!("Opening socket {} for incoming TCP clients", config.listen_socket());
    let socket = TcpListener::bind(config.listen_socket()).expect("Failed to bind socket");

    info!("Server started, waiting for clients");
    let done = socket.incoming().for_each(move |socket| {
        info!("Accepted client connection, serving requests");

        let server_clone = server.clone();

        // TODO fill this in properly for each connection based on TLS authentication info
        let handshake_fut = handshake::temporary_unsafe_tcp_handshake_until_diffie_hellman_done(
            socket,
            signer.clone(),
        )
        .map_err(|e| warn!("Client handshake failed: {:?}", e))
        .and_then(move |(reader, writer, client_context)| {
            let home = HomeConnectionServer::new(Rc::new(client_context), server_clone.clone())
                .map_err(|e| warn!("Failed to create server instance: {:?}", e))?;
            HomeDispatcherCapnProto::dispatch(Rc::new(home), reader, writer);
            Ok(())
        });

        reactor::spawn(handshake_fut);
        Ok(())
    });

    let res = reactor.block_on(done);
    debug!("Reactor finished with result: {:?}", res);
    info!("Server shutdown");
}
