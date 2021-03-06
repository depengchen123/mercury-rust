use std::convert::TryFrom;
use std::rc::Rc;

use capnp::capability::Promise;
use capnp_rpc::pry;
use futures::{Future, Stream};
use tokio::io::AsyncRead;
use tokio::net::tcp::TcpStream;
use tokio_current_thread as reactor;

use super::*;
use crate::mercury_capnp::{capnp_err, FillFrom};

pub struct HomeDispatcherCapnProto {
    home: Rc<dyn Home>,
    // TODO probably we should have a SessionFactory here instead of instantiating sessions "manually"
}

impl HomeDispatcherCapnProto {
    // TODO how to access PeerContext in the Home implementation?
    pub fn dispatch<R, W>(home: Rc<dyn Home>, reader: R, writer: W)
    where
        R: std::io::Read + 'static,
        W: std::io::Write + 'static,
    {
        let dispatcher = Self { home };

        let home_capnp =
            mercury_capnp::home::ToClient::new(dispatcher).into_client::<::capnp_rpc::Server>();
        let network = capnp_rpc::twoparty::VatNetwork::new(
            reader,
            writer,
            capnp_rpc::rpc_twoparty_capnp::Side::Server,
            Default::default(),
        );

        let rpc_system =
            capnp_rpc::RpcSystem::new(Box::new(network), Some(home_capnp.clone().client));

        reactor::spawn(rpc_system.map_err(|e| warn!("Capnp RPC failed: {}", e)));
    }

    pub fn dispatch_tcp(home: Rc<dyn Home>, tcp_stream: TcpStream) {
        tcp_stream.set_nodelay(true).unwrap();
        let (reader, writer) = tcp_stream.split();
        HomeDispatcherCapnProto::dispatch(home, reader, writer)
    }
}

// NOTE useful for testing connection lifecycles
impl Drop for HomeDispatcherCapnProto {
    fn drop(&mut self) {
        debug!("Home connection dropped");
    }
}

impl mercury_capnp::profile_repo::Server for HomeDispatcherCapnProto {
    fn get(
        &mut self,
        params: mercury_capnp::profile_repo::GetParams,
        mut results: mercury_capnp::profile_repo::GetResults,
    ) -> Promise<(), capnp::Error> {
        let profile_id_capnp = pry!(pry!(params.get()).get_profile_id());
        let profile_id = pry!(ProfileId::from_bytes(profile_id_capnp).map_err(|e| capnp_err(e)));
        let load_fut = self
            .home
            .fetch(&profile_id)
            .map(move |profile| results.get().set_profile(&profile_to_bytes(&profile)))
            .map_err(|e| capnp::Error::failed(format!("Failed to load profile id: {:?}", e)));

        Promise::from_future(load_fut)
    }
}

impl mercury_capnp::home::Server for HomeDispatcherCapnProto {
    fn claim(
        &mut self,
        params: mercury_capnp::home::ClaimParams,
        mut results: mercury_capnp::home::ClaimResults,
    ) -> Promise<(), capnp::Error> {
        let profile_id_capnp = pry!(pry!(params.get()).get_profile_id());
        let profile_id = pry!(ProfileId::from_bytes(profile_id_capnp).map_err(|e| capnp_err(e)));
        let claim_fut = self
            .home
            .claim(profile_id)
            .map_err(|e| capnp::Error::failed(format!("Failed to claim profile: {:?}", e)))
            .map(move |hosting_proof| results.get().init_hosting_proof().fill_from(&hosting_proof));

        Promise::from_future(claim_fut)
    }

    fn register(
        &mut self,
        params: mercury_capnp::home::RegisterParams,
        mut results: mercury_capnp::home::RegisterResults,
    ) -> Promise<(), capnp::Error> {
        let half_proof_capnp = pry!(pry!(params.get()).get_half_proof());
        let half_proof = pry!(RelationHalfProof::try_from(half_proof_capnp));

        //let inv_capnp_res = pry!(params.get()).get_invite();
        //let invite_opt =
        //    inv_capnp_res.and_then(|inv_capnp| HomeInvitation::try_from(inv_capnp)).ok();

        let reg_fut = self
            .home
            .register(half_proof) //, invite_opt)
            .map_err(|e| capnp::Error::failed(format!("Failed to register profile: {:?}", e)))
            .map(move |proof| results.get().init_hosting_proof().fill_from(&proof));

        Promise::from_future(reg_fut)
    }

    fn login(
        &mut self,
        params: mercury_capnp::home::LoginParams,
        mut results: mercury_capnp::home::LoginResults,
    ) -> Promise<(), capnp::Error> {
        //let profile_id = pry!( pry!( params.get() ).get_profile_id() );
        let host_proof_capnp = pry!(pry!(params.get()).get_hosting_proof());
        let host_proof = pry!(RelationProof::try_from(host_proof_capnp));
        let session_fut = self
            .home
            .login(&host_proof)
            .map(move |session_impl| {
                let session_dispatcher = HomeSessionDispatcherCapnProto::new(session_impl);
                let session = mercury_capnp::home_session::ToClient::new(session_dispatcher)
                    .into_client::<capnp_rpc::Server>();
                results.get().set_session(session);
                ()
            })
            .map_err(|e| capnp::Error::failed(format!("Failed to login: {:?}", e)));

        Promise::from_future(session_fut)
    }

    fn pair_request(
        &mut self,
        params: mercury_capnp::home::PairRequestParams,
        mut _results: mercury_capnp::home::PairRequestResults,
    ) -> Promise<(), capnp::Error> {
        let half_proof_capnp = pry!(pry!(params.get()).get_half_proof());
        let half_proof = pry!(RelationHalfProof::try_from(half_proof_capnp));

        let pair_req_fut = self
            .home
            .pair_request(half_proof)
            .map_err(|e| capnp::Error::failed(format!("Failed to request pairing {:?}", e)));

        Promise::from_future(pair_req_fut)
    }

    fn pair_response(
        &mut self,
        params: mercury_capnp::home::PairResponseParams,
        mut _results: mercury_capnp::home::PairResponseResults,
    ) -> Promise<(), capnp::Error> {
        let proof_capnp = pry!(pry!(params.get()).get_relation());
        let proof = pry!(RelationProof::try_from(proof_capnp));

        let pair_resp_fut = self
            .home
            .pair_response(proof)
            .map_err(|e| capnp::Error::failed(format!("Failed to send pairing response: {:?}", e)));

        Promise::from_future(pair_resp_fut)
    }

    fn call(
        &mut self,
        params: mercury_capnp::home::CallParams,
        mut results: mercury_capnp::home::CallResults,
    ) -> Promise<(), capnp::Error> {
        let opts = pry!(params.get());
        let rel_capnp = pry!(opts.get_relation());
        let app_capnp = pry!(opts.get_app());
        let init_payload_capnp = pry!(opts.get_init_payload());

        let to_caller = opts
            .get_to_caller()
            .map(|to_caller_capnp| mercury_capnp::fwd_appmsg(to_caller_capnp))
            .ok();

        let relation = pry!(RelationProof::try_from(rel_capnp));
        let app = ApplicationId::from(app_capnp);
        let init_payload = AppMessageFrame::from(init_payload_capnp);

        let call_req = CallRequestDetails { relation, init_payload, to_caller };
        let call_fut = self
            .home
            .call(app, call_req)
            .map(|to_callee_opt| {
                to_callee_opt.map(move |to_callee| {
                    let to_callee_dispatch =
                        mercury_capnp::AppMessageDispatcherCapnProto::new(to_callee);
                    let to_callee_capnp =
                        mercury_capnp::app_message_listener::ToClient::new(to_callee_dispatch)
                            .into_client::<::capnp_rpc::Server>();
                    results.get().set_to_callee(to_callee_capnp);
                });
            })
            .map_err(|e| capnp::Error::failed(format!("Failed to call profile: {:?}", e)));

        Promise::from_future(call_fut)
    }
}

pub struct HomeSessionDispatcherCapnProto {
    session: Rc<dyn HomeSession>,
}

impl HomeSessionDispatcherCapnProto {
    pub fn new(session: Rc<dyn HomeSession>) -> Self {
        Self { session }
    }
}

// NOTE useful for testing connection lifecycles
impl Drop for HomeSessionDispatcherCapnProto {
    fn drop(&mut self) {
        debug!("Session over Home connection dropped");
    }
}

impl mercury_capnp::home_session::Server for HomeSessionDispatcherCapnProto {
    fn backup(
        &mut self,
        params: mercury_capnp::home_session::BackupParams,
        mut _results: mercury_capnp::home_session::BackupResults,
    ) -> Promise<(), capnp::Error> {
        let own_profile_capnp = pry!(pry!(params.get()).get_own_profile());
        let own_profile = pry!(bytes_to_own_profile(own_profile_capnp));

        let upd_fut = self
            .session
            .backup(own_profile)
            .map_err(|e| capnp::Error::failed(format!("Failed to update profile: {:?}", e)));

        Promise::from_future(upd_fut)
    }

    fn restore(
        &mut self,
        _params: mercury_capnp::home_session::RestoreParams,
        mut results: mercury_capnp::home_session::RestoreResults,
    ) -> Promise<(), capnp::Error> {
        let upd_fut = self
            .session
            .restore()
            .map(|own_prof| own_profile_to_bytes(&own_prof))
            .map(move |own_bytes| results.get().set_own_profile(&own_bytes))
            .map_err(|e| capnp::Error::failed(format!("Failed to update profile: {:?}", e)));

        Promise::from_future(upd_fut)
    }

    fn unregister(
        &mut self,
        params: mercury_capnp::home_session::UnregisterParams,
        mut _results: mercury_capnp::home_session::UnregisterResults,
    ) -> Promise<(), capnp::Error> {
        let new_home_res_capnp = pry!(params.get()).get_new_home();
        let new_home_opt =
            new_home_res_capnp.and_then(|new_home_capnp| bytes_to_profile(&new_home_capnp)).ok();

        let upd_fut = self
            .session
            .unregister(new_home_opt)
            .map_err(|e| capnp::Error::failed(format!("Failed to unregister profile: {:?}", e)));

        Promise::from_future(upd_fut)
    }

    fn ping(
        &mut self,
        params: mercury_capnp::home_session::PingParams,
        mut results: mercury_capnp::home_session::PingResults,
    ) -> Promise<(), capnp::Error> {
        let txt = pry!(pry!(params.get()).get_txt());
        let ping_fut = self
            .session
            .ping(txt)
            .map_err(|e| capnp::Error::failed(format!("Failed ping: {:?}", e)))
            .map(move |pong| results.get().set_pong(&pong));
        Promise::from_future(ping_fut)
    }

    fn events(
        &mut self,
        params: mercury_capnp::home_session::EventsParams,
        mut _results: mercury_capnp::home_session::EventsResults,
    ) -> Promise<(), capnp::Error> {
        let callback = pry!(pry!(params.get()).get_event_listener());
        let events_fut = self
            .session
            .events()
            .map_err(|e| capnp::Error::failed(format!("Failed to get profile events: {:?}", e)))
            .for_each(move |item| {
                debug!("Capnp server is forwarding event to the client: {:?}", item);
                match item {
                    Ok(event) => {
                        let mut request = callback.receive_request();
                        request.get().init_event().fill_from(&event);
                        let fut = request.send().promise.map(|_resp| ());
                        // TODO .map_err() what to do here in case of an error?
                        Box::new(fut) as AsyncResult<(), capnp::Error>
                    }
                    Err(err) => {
                        let mut request = callback.error_request();
                        request.get().set_error(&err);
                        let fut = request.send().promise.map(|_resp| ());
                        // TODO .map_err() what to do here in case of an error?
                        Box::new(fut)
                    }
                }
            });

        Promise::from_future(events_fut)
    }

    fn checkin_app(
        &mut self,
        params: mercury_capnp::home_session::CheckinAppParams,
        _results: mercury_capnp::home_session::CheckinAppResults,
    ) -> Promise<(), capnp::Error> {
        // Receive a proxy from client to which the server will send notifications on incoming calls
        let params = pry!(params.get());
        let app_id = pry!(params.get_app());
        let call_listener = pry!(params.get_call_listener());

        // Forward incoming calls from business logic into capnp proxy stub of client
        let calls_fut = self.session.checkin_app( &app_id.into() )
            .map_err( | e| capnp::Error::failed( format!("Failed to checkin app: {:?}", e) ) )
            .for_each( move |item|
            {
                match item
                {
                    Ok(incoming_call) =>
                    {
                        let mut request = call_listener.receive_request();
                        request.get().init_call().fill_from( incoming_call.request_details() );

                        if let Some(ref to_caller) = incoming_call.request_details().to_caller
                        {
                            // Set up a capnp channel to the caller for the callee
                            let listener = mercury_capnp::AppMessageDispatcherCapnProto::new(to_caller.clone() );
                            // TODO consider how to drop/unregister this object from capnp if the stream is dropped
                            let listener_capnp = mercury_capnp::app_message_listener::ToClient::new(listener)
                                .into_client::<::capnp_rpc::Server>();
                            request.get().get_call().expect("Implementation error: call was just initialized above, should be there")
                                .set_to_caller(listener_capnp);
                        }

                        let fut = request.send().promise
                            .map( move |resp|
                            {
                                let answer = resp.get()
                                    .and_then( |res| res.get_to_callee() )
                                    .map( |to_callee_capnp| mercury_capnp::fwd_appmsg(to_callee_capnp) )
                                    .map_err( |e| e ) // TODO should we something about errors here?
                                    .ok();
                                incoming_call.answer(answer);
                            } );
                        Box::new(fut) as AsyncResult<(), capnp::Error>
                    },
                    Err(err) =>
                    {
                        let mut request = call_listener.error_request();
                        request.get().set_error(&err);
                        let fut = request.send().promise
                            .map( | _resp| () );
                        Box::new(fut)
                    },
                }
            } );

        Promise::from_future(calls_fut)
    }
}
