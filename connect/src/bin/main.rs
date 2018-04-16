#![allow(unused)]
extern crate mercury_connect;
extern crate mercury_home_protocol;

extern crate multihash;
extern crate multiaddr;

extern crate tokio_stdin_stdout;
extern crate tokio_core;
extern crate tokio_io;
extern crate futures;

mod dummy;

use dummy::{ProfileStore, MyDummyHome};

use std::cell::RefCell;
use std::rc::Rc;
use std::io::{BufRead, Read, Write, stdin};

use mercury_connect::*;
use mercury_home_protocol::*;
use ::net::*;
use ::mock::*;

use multihash::{encode, Hash};
use multiaddr::{Multiaddr, ToMultiaddr};

use tokio_core::reactor;
use tokio_core::net::TcpStream;
use tokio_io::{AsyncRead, AsyncWrite};
use futures::{Future,Stream};

fn main(){
    //print!("{}[2J", 27 as char);
    println!("Setting up config");
    let mut reactor = reactor::Core::new().unwrap();
    let mut reactorhandle = reactor.handle();
    let homeaddr = "/ip4/127.0.0.1/udp/9876";
    let homemultiaddr = homeaddr.to_multiaddr().unwrap();
    
    println!("Setting up signers");
    let signo = Rc::new(mock::Signo::new("Deuszkulcs"));
    let homesigno = Rc::new(mock::Signo::new("makusguba"));
    
    println!("Setting up home");
    let home_id = ProfileId(mock::generate_hash("home"));
    let home_pubkey = PublicKey(Vec::from("home public key"));
    let homeprof = Profile::new_home(home_id.clone(), home_pubkey.clone(), homemultiaddr.clone());
    
    let mut profile = make_own_persona_profile("Deusz", signo.pub_key());
    
    println!("Setting up connection");

    let mut home_storage = Rc::new( ProfileStore::new() );
    let mut home = Rc::new( RefCell::new( MyDummyHome::new( homeprof.clone() , home_storage) ) );

    // Rc::get_mut(&mut home).unwrap().insert(id , homeprof.clone());

    let mut dht = ProfileStore::new();
    dht.insert(home.borrow().home_profile.id.clone(), homeprof.clone());

    let profilegateway = ProfileGatewayImpl{
        signer:         signo,
        profile_repo:   Rc::new(dht),
        home_connector: Rc::new( dummy::DummyConnector::new_with_home( home ) ),
    };

    // // let bizbasz = TcpStream::connect( &multiaddr_to_socketaddr(&homemultiaddr).unwrap() , &reactorhandle.clone() )
    // // .map(|stream|{

    // // });
    // // let appcontext = reactor.run(bizbasz).unwrap();

    println!("Registering");
    let reg = profilegateway.register(home_id, mock::create_ownprofile( profile ), None);
    let ownprofile = reactor.run(reg).unwrap();
    println!("{:?}",ownprofile );
    
    println!("Logging in");
    println!("Getting session");
    let session = reactor.run( profilegateway.login() ).unwrap();
    
    println!("All set up");
    
    println!("Menu\n1. Connect\n2. Call(crashes)\n3. Pair\n4. Ping\n5. Show profile\nExit with ctrl+d");
    let mut buffer = String::new();
    let stdin = tokio_stdin_stdout::stdin(1);
    let bufreader = std::io::BufReader::new(stdin);
    let instream = tokio_io::io::lines(bufreader);
    let stdin_closed = instream.for_each(|line|{     
        match line.as_ref(){
            "1" =>{
                let signer = profilegateway.signer.to_owned();
                profilegateway.home_connector.connect(&homeprof, signer);
                println!("connect");
    
            },
            //call dies miserably 
            "2" =>{
                profilegateway.call(
                    mock::dummy_relation("work"), 
                    ApplicationId( String::from("SampleApp") ), 
                    AppMessageFrame("whatever".as_bytes().to_owned() ) 
                );
    
            }
            "3" =>{
                profilegateway.pair_request("relation_dummy_type", "url");
    
            }
            "4" =>{
                session.ping("dummy_ping");
    
            }
            "5" =>{
                println!("{:?}", ownprofile);
    
            }
            _ =>{
                println!("nope");
    
            },
        };
        futures::future::ok::<(),std::io::Error>(())
    });
    reactor.run(stdin_closed);
}
