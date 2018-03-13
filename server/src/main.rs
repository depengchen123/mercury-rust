extern crate capnp;
extern crate capnp_rpc;
extern crate futures;
extern crate mercury_common;
extern crate multiaddr;
extern crate multihash;
extern crate tokio_core;
extern crate tokio_io;

use capnp::capability::Promise;
use futures::{Future, Stream};
use mercury_common::mercury_capnp;
use tokio_core::reactor;
use tokio_core::net::TcpListener;
use tokio_io::AsyncRead;



struct HomeImpl {}

impl HomeImpl
{
    pub fn new() -> Self { Self{} }
}

impl mercury_capnp::home::Server for HomeImpl
{
    fn ping(&mut self,
             params: mercury_capnp::home::PingParams,
             mut results: mercury_capnp::home::PingResults,)
        -> Promise<(), ::capnp::Error>
    {
        let res = params.get()
            .and_then( |params| params.get_txt() )
            .and_then( |txt|
            {
                println!("ping called with '{}', sending pong", txt);
                Ok(())
            } );

        results.get().set_result(&"wooooorks");

        match res {
            Ok(_) => Promise::ok(()),
            Err(e) => Promise::err(e),
        }

    }
}



fn main()
{
    let mut core = reactor::Core::new().unwrap();
    let handle = core.handle();

    use std::net::ToSocketAddrs;
    let addr = "localhost:9876".to_socket_addrs().unwrap().next().expect("Failed to parse address");
    let socket = TcpListener::bind(&addr, &handle).expect("Failed to bind socket");

    let home_impl = HomeImpl::new();
    let home = mercury_capnp::home::ToClient::new(home_impl)
        .from_server::<::capnp_rpc::Server>();

    println!("Waiting for clients");
    let handle1 = handle.clone();
    let done = socket.incoming().for_each(move |(socket, _addr)|
    {
        println!("Accepted client connection, serving requests");
        try!(socket.set_nodelay(true));
        let (reader, writer) = socket.split();
        let handle = handle1.clone();

        let network = capnp_rpc::twoparty::VatNetwork::new( reader, writer,
            capnp_rpc::rpc_twoparty_capnp::Side::Server, Default::default() );

        let rpc_system = capnp_rpc::RpcSystem::new( Box::new(network), Some(home.clone().client) );

        handle.spawn(rpc_system.map_err(|_| ()));
        Ok(())
    } );

    core.run(done).unwrap();
}
