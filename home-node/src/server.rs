use std::collections::HashMap;
use std::time::Duration;
use std::{cell::RefCell, rc::Rc, rc::Weak};

use failure::Fail;
use futures::sync::{mpsc, oneshot};
use futures::{future, stream, Future, Sink};
use log::*;
use tokio_core::reactor::{self, Timeout};

use claims::model::Link;
use mercury_home_protocol::api::AsyncSink; // TODO this should normally work with protocol::*, why is this needed?
use mercury_home_protocol::error::*;
use mercury_home_protocol::*;

// TODO this should come from user configuration with a reasonable default value close to this
const CFG_CALL_ANSWER_TIMEOUT: Duration = Duration::from_secs(30);

pub struct HomeServer {
    handle: reactor::Handle,
    validator: Rc<Validator>,
    public_profile_dht: Rc<RefCell<DistributedPublicProfileRepository>>,
    hosted_profile_db: Rc<RefCell<PrivateProfileRepository>>,
    sessions: Rc<RefCell<HashMap<ProfileId, Weak<HomeSessionServer>>>>,
}

impl HomeServer {
    pub fn new(
        handle: &reactor::Handle,
        validator: Rc<Validator>,
        public_dht: Rc<RefCell<DistributedPublicProfileRepository>>,
        private_db: Rc<RefCell<PrivateProfileRepository>>,
    ) -> Self {
        Self {
            handle: handle.clone(),
            validator,
            public_profile_dht: public_dht,
            hosted_profile_db: private_db,
            sessions: Rc::new(RefCell::new(HashMap::new())),
        }
    }
}

pub struct HomeConnectionServer {
    server: Rc<HomeServer>, // TODO consider if we should have a RefCell<> for mutability here
    context: Rc<PeerContext>,
}

impl HomeConnectionServer {
    pub fn new(context: Rc<PeerContext>, server: Rc<HomeServer>) -> Result<Self, Error> {
        context
            .validate(&*server.validator)
            .map_err(|err| err.context(ErrorKind::ContextValidationFailed))?;
        Ok(Self { context, server })
    }

    /// Returns Error if the profile is not hosted on this home server
    /// Returns None if the profile is not online
    fn get_live_session(
        server: Rc<HomeServer>,
        to_profile: ProfileId,
    ) -> Box<Future<Item = Option<Rc<HomeSessionServer>>, Error = Error>> {
        let sessions_clone = server.sessions.clone();

        // Check if this profile is hosted on this server
        let session_fut = server
            .hosted_profile_db
            .borrow()
            .get(&to_profile)
            .and_then(move |_profile_data| {
                // Seperate variable needed, see https://stackoverflow.com/questions/50391668/running-asynchronous-mutable-operations-with-rust-futures
                let sessions = sessions_clone.borrow();
                // If hosted here, check if profile is in reach with an online session
                let session_rc = sessions.get(&to_profile).and_then(|weak| weak.upgrade());
                future::ok(session_rc)
            })
            .map_err(|err| err.context(ErrorKind::FailedToGetSession).into());

        Box::new(session_fut)
    }

    fn push_event(
        server: Rc<HomeServer>,
        to_profile: ProfileId,
        event: ProfileEvent,
    ) -> Box<Future<Item = (), Error = Error>> {
        let push_fut = Self::get_live_session(server, to_profile).and_then(|session_rc_opt| {
            match session_rc_opt {
                // TODO if push to session fails, consider just dropping the session
                //      (is anything manual needed using weak pointers?) and requiring a reconnect
                Some(ref session) => session.push_event(event),
                // TODO save event into persistent storage and delegate it when profile is online again
                None => Box::new(future::ok(())),
            }
        });

        Box::new(push_fut)
    }

    fn push_call(
        server: Rc<HomeServer>,
        to_profile: ProfileId,
        to_app: ApplicationId,
        call: Box<IncomingCall>,
    ) -> Box<Future<Item = (), Error = Error>> {
        let push_fut = Self::get_live_session(server, to_profile).and_then(|session_rc_opt| {
            match session_rc_opt {
                Some(ref session) => {
                    // TODO if push to session fails, consider just dropping the session
                    //      (is anything manual needed using weak pointers?) and requiring a reconnect
                    let push_fut = session.push_call(to_app, call);
                    Box::new(push_fut) as Box<Future<Item = (), Error = Error>>
                }
                // TODO save event into persistent storage and delegate it when profile is online again
                None => Box::new(future::ok(())),
            }
        });

        Box::new(push_fut)
    }
}

impl ProfileExplorer for HomeConnectionServer {
    fn fetch(&self, id: &ProfileId) -> AsyncFallible<Profile> {
        let profile_fut = self
            .server
            .public_profile_dht
            .borrow()
            .get_public(id)
            .map_err(|e| e.context(ErrorKind::DhtLookupFailed).into());
        Box::new(profile_fut)
    }

    fn followers(&self, _id: &ProfileId) -> AsyncFallible<Vec<Link>> {
        unimplemented!()
        // Ok( Vec::new() ) // TODO implement this properly
    }
}

impl Home for HomeConnectionServer {
    fn claim(&self, profile: ProfileId) -> Box<Future<Item = OwnProfile, Error = Error>> {
        if profile != self.context.peer_id() {
            return Box::new(future::err(ErrorKind::FailedToClaimProfile.into()));
        }

        let claim_fut = self
            .server
            .hosted_profile_db
            .borrow()
            .get(&profile)
            .map_err(|e| e.context(ErrorKind::FailedToClaimProfile).into());
        Box::new(claim_fut)
    }

    fn register(
        &self,
        own_prof: OwnProfile,
        half_proof: RelationHalfProof,
        //_invite: Option<HomeInvitation>,
    ) -> Box<Future<Item = OwnProfile, Error = (OwnProfile, Error)>> {
        if own_prof.id() != self.context.peer_id() {
            return Box::new(future::err((own_prof, ErrorKind::ProfileMismatch.into())));
        }

        if own_prof.public_key() != self.context.peer_pubkey() {
            return Box::new(future::err((own_prof, ErrorKind::PublicKeyMismatch.into())));
        }

        if half_proof.signer_id != self.context.peer_id() {
            return Box::new(future::err((own_prof, ErrorKind::SignerMismatch.into())));
        }

        trace!(
            "Request was sent for home_id: {:?}, this should be me, i.e. match my id: {:?}",
            half_proof.peer_id,
            self.context.my_signer().profile_id()
        );
        if half_proof.peer_id != self.context.my_signer().profile_id() {
            return Box::new(future::err((own_prof, ErrorKind::HomeIdMismatch.into())));
        }

        if half_proof.relation_type != RelationProof::RELATION_TYPE_HOSTED_ON_HOME {
            return Box::new(future::err((own_prof, ErrorKind::RelationTypeMismatch.into())));
        }

        if self
            .server
            .validator
            .validate_half_proof(&half_proof, &self.context.peer_pubkey())
            .is_err()
        {
            return Box::new(future::err((own_prof, ErrorKind::InvalidSignature.into())));
        }

        let own_prof_clone = own_prof.clone();
        let error_mapper =
            move |e: failure::Error| (own_prof_clone, ErrorKind::StorageFailed.into());
        let error_mapper_clone = error_mapper.clone();

        let home_proof =
            match RelationProof::sign_remaining_half(&half_proof, self.context.my_signer()) {
                Err(e) => return Box::new(future::err((own_prof, e))),
                Ok(proof) => proof,
            };

        let pub_prof = own_prof.public_data();
        let own_prof_modified = match pub_prof.as_persona() {
            // TODO the registration request (including adding the new home and increasing version)
            //      will have to be signed by the client on the long run,
            //      thus cannot be done by the server here, to be migrated to the client code.
            Some(ref mut persona_facet) => {
                // TODO this should be shorter, just adds a new home but rebuilds a lot of data structure
                persona_facet.homes.push(home_proof);
                let attributes = persona_facet.to_attributes();
                let profile = Profile::new(
                    pub_prof.public_key(),
                    pub_prof.version() + 1,
                    pub_prof.links().to_owned(),
                    attributes,
                );
                OwnProfile::new(profile, own_prof.private_data())
            }
            None => return Box::new(future::err((own_prof, ErrorKind::PersonaExpected.into()))),
        };

        let pub_prof_modified = own_prof_modified.public_data();
        let local_store = self.server.hosted_profile_db.clone();
        let distributed_store = self.server.public_profile_dht.clone();
        let reg_fut = self
            .server
            .hosted_profile_db
            .borrow()
            .get(&own_prof.id())
            .then(|get_res| {
                match get_res {
                    Ok(_stored_prof) => {
                        debug!("Profile was already registered");
                        Err((own_prof, ErrorKind::AlreadyRegistered.into()))
                    }
                    // TODO only errors like NotFound should be accepted here but other (e.g. I/O) errors should be delegated
                    Err(_e) => Ok(()),
                }
            })
            // NOTE Block with "return" is needed, see https://stackoverflow.com/questions/50391668/running-asynchronous-mutable-operations-with-rust-futures
            .and_then(move |_| {
                // Store public profile parts in distributed storage (e.g. DHT)
                debug!("Saving public profile info into distributed storage");
                return distributed_store
                    .borrow_mut()
                    .set_public(pub_prof_modified)
                    .map_err(error_mapper_clone);
            })
            .and_then(move |_| {
                // Store private profile info in local storage only (e.g. SQL)
                debug!("Saving private profile info into local storage");
                return local_store
                    .borrow_mut()
                    .set(own_prof_modified.clone())
                    .map(|_| own_prof_modified)
                    .map_err(error_mapper);
            });

        Box::new(reg_fut)
    }

    fn login(
        &self,
        proof_of_home: &RelationProof,
    ) -> Box<Future<Item = Rc<HomeSession>, Error = Error>> {
        if *proof_of_home.relation_type != *RelationProof::RELATION_TYPE_HOSTED_ON_HOME {
            return Box::new(future::err(ErrorKind::RelationTypeMismatch.into()));
        }

        let profile_id = match proof_of_home.peer_id(&self.context.my_signer().profile_id()) {
            Ok(profile_id) => profile_id.to_owned(),
            Err(e) => return Box::new(future::err(e.context(ErrorKind::ProfileMismatch).into())),
        };

        let val_fut = self
            .server
            .hosted_profile_db
            .borrow()
            .get(&profile_id)
            .map({
                let context_clone = self.context.clone();
                let server_clone = self.server.clone();
                let sessions_clone = self.server.sessions.clone();
                move |_own_profile| {
                    let session = Rc::new(HomeSessionServer::new(context_clone, server_clone));
                    sessions_clone
                        .borrow_mut()
                        .entry(profile_id)
                        .or_insert(Rc::downgrade(&session));
                    session as Rc<HomeSession>
                }
            })
            .map_err(|e| e.context(ErrorKind::FailedToLoadProfile).into());

        Box::new(val_fut)
    }

    fn pair_request(&self, half_proof: RelationHalfProof) -> Box<Future<Item = (), Error = Error>> {
        if half_proof.signer_id != self.context.peer_id() {
            return Box::new(future::err(ErrorKind::ProfileMismatch.into()));
        }

        if self
            .server
            .validator
            .validate_half_proof(&half_proof, &self.context.peer_pubkey())
            .is_err()
        {
            return Box::new(future::err(ErrorKind::PublicKeyMismatch.into()));
        }

        let to_profile = half_proof.peer_id.clone();
        Self::push_event(self.server.clone(), to_profile, ProfileEvent::PairingRequest(half_proof))
    }

    fn pair_response(&self, relation: RelationProof) -> Box<Future<Item = (), Error = Error>> {
        let to_profile = match relation.peer_id(&self.context.peer_id()) {
            Ok(profile_id) => profile_id.to_owned(),
            Err(err) => {
                return Box::new(future::err(err.context(ErrorKind::ProfileMismatch).into()));
            }
        };

        let server_clone = self.server.clone();
        let server_clone2 = self.server.clone();
        let peer_id_clone = self.context.peer_id().clone();
        let peer_pubkey_clone = self.context.peer_pubkey().clone();
        let relation_clone = relation.clone();

        // We need to look up the public key to be able to validate the proof
        let fut = self
            .server
            .hosted_profile_db
            .borrow()
            .get(&to_profile)
            .map_err(|err| err.context(ErrorKind::PeerNotHostedHere).into())
            .and_then(move |profile_data| {
                server_clone
                    .validator
                    .validate_relation_proof(
                        &relation,
                        &peer_id_clone,
                        &peer_pubkey_clone,
                        &profile_data.id(),
                        &profile_data.public_key(),
                    )
                    .map_err(|err| err.context(ErrorKind::InvalidRelationProof).into())
            })
            .and_then(|_| {
                Self::push_event(
                    server_clone2,
                    to_profile,
                    ProfileEvent::PairingResponse(relation_clone),
                )
            });

        Box::new(fut)
    }

    fn call(
        &self,
        app: ApplicationId,
        call_req: CallRequestDetails,
    ) -> Box<Future<Item = Option<AppMsgSink>, Error = Error>> {
        // TODO add error case for calling self
        let to_profile = match call_req.relation.peer_id(&self.context.peer_id()) {
            Ok(profile_id) => profile_id.to_owned(),
            Err(e) => return Box::new(future::err(e.context(ErrorKind::ProfileMismatch).into())),
        };

        let server_clone = self.server.clone();
        let server_clone2 = self.server.clone();
        let peer_id_clone = self.context.peer_id().clone();
        let peer_pubkey_clone = self.context.peer_pubkey().clone();
        let relation = call_req.relation.clone();
        let (send, recv) = oneshot::channel();
        let call = Box::new(Call::new(call_req, send));
        let handle = self.server.handle.clone();

        let timeout_fut = match Timeout::new(CFG_CALL_ANSWER_TIMEOUT, &handle) {
            Ok(timeout_fut) => {
                timeout_fut.map(|_| None).map_err(|e| e.context(ErrorKind::TimeoutFailed).into())
            }
            Err(err) => return Box::new(future::err(err.context(ErrorKind::TimeoutFailed).into())),
        };

        let answer_fut = self
            .server
            .hosted_profile_db
            .borrow()
            .get(&to_profile)
            .map_err(|e| e.context(ErrorKind::PeerNotHostedHere).into())
            .and_then(move |profile_data| {
                server_clone
                    .validator
                    .validate_relation_proof(
                        &relation,
                        &peer_id_clone,
                        &peer_pubkey_clone,
                        &profile_data.id(),
                        &profile_data.public_key(),
                    )
                    .map_err(|err| err.context(ErrorKind::InvalidRelationProof).into())
            })
            .and_then(|_| {
                Self::push_call(server_clone2, to_profile, app, call)
                    .map_err(|err| err.context(ErrorKind::CallFailed).into())
            })
            .and_then(move |_void| {
                let answer_fut =
                    recv.map_err(|e| e.context(ErrorKind::FailedToReadResponse).into());

                // Wait for answer with specified timeout
                answer_fut
                    .select(timeout_fut)
                    .map(|(done, _pending)| done)
                    .map_err(|(e, _pending)| e)
            });
        Box::new(answer_fut)
    }
}

struct Call {
    request: CallRequestDetails,
    sender: oneshot::Sender<Option<AppMsgSink>>,
}

impl Call {
    pub fn new(request: CallRequestDetails, sender: oneshot::Sender<Option<AppMsgSink>>) -> Self {
        Self { request, sender }
    }
}

impl IncomingCall for Call {
    fn request_details(&self) -> &CallRequestDetails {
        &self.request
    }
    fn answer(self: Box<Self>, to_callee: Option<AppMsgSink>) -> CallRequestDetails {
        // NOTE needed to dereference Box because otherwise the whole self is moved at its first dereference
        let this = *self;
        if let Err(e) = this.sender.send(to_callee) {
            // TODO We should at least log the error here.
            //      To solve this better, the function probably should return a Result<T,E> instead of T.
        }
        this.request
    }
}

enum ServerSink<T, E> {
    // TODO message buffer should be persistent on the long run
    Buffer(Vec<Result<T, E>>), // Temporary buffer, sink is not initialized
    Sender(AsyncSink<T, E>), // Initialized sink end of channel, user is listening on the other half
}

pub struct HomeSessionServer {
    // TODO consider using Weak<Ptrs> instead of Rc<Ptrs> if a closed Home connection cannot
    //      drop all related sessions automatically
    context: Rc<PeerContext>,
    server: Rc<HomeServer>,
    events: RefCell<ServerSink<ProfileEvent, String>>,
    apps: RefCell<HashMap<ApplicationId, ServerSink<Box<IncomingCall>, String>>>, // {appId->sender<call>}
}

impl HomeSessionServer {
    // TODO consider if validating the context is needed here, e.g. as an assert()
    pub fn new(context: Rc<PeerContext>, server: Rc<HomeServer>) -> Self {
        Self {
            context,
            server,
            events: RefCell::new(ServerSink::Buffer(Vec::new())),
            apps: RefCell::new(HashMap::new()),
        }
    }

    fn push_event(&self, event: ProfileEvent) -> Box<Future<Item = (), Error = Error>> {
        debug!("Session with {} got event dispatched: {:?}", self.context.peer_id(), event);
        match *self.events.borrow_mut() {
            ServerSink::Buffer(ref mut bufvec) => {
                debug!("No event channel is available on the client side, buffering event");
                bufvec.push(Ok(event)); // TODO consider size constraints
                Box::new(future::ok(()))
            }
            ServerSink::Sender(ref mut sender) => {
                debug!("Event channel for active client was found, sending event there");
                let fut = sender.clone()
                    .send(Ok(event))
                    .map(|_sender| ())
                    // TODO if call dispatch fails we probably should replace the sender with a buffer
                    .map_err(|e| ErrorKind::FailedToSend.into());
                Box::new(fut)
            }
        }
    }

    fn push_call(
        &self,
        app: ApplicationId,
        call: Box<IncomingCall>,
    ) -> Box<Future<Item = (), Error = Error>> {
        debug!(
            "Session with {} dispatched call with relation: {:?}",
            self.context.peer_id(),
            call.request_details().relation
        );
        let mut apps = self.apps.borrow_mut();
        let sink = apps.entry(app).or_insert(ServerSink::Buffer(Vec::new()));
        match *sink {
            ServerSink::Buffer(ref mut bufvec) => {
                bufvec.push(Ok(call)); // TODO consider size constraints
                Box::new(future::ok(()))
            }
            ServerSink::Sender(ref mut sender) => Box::new(
                sender
                    .clone()
                    .send(Ok(call))
                    .map(|_sender| ())
                    // TODO if call dispatch fails we probably should replace the sender with a buffer
                    .map_err(|e| ErrorKind::FailedToSend.into()),
            ),
        }
    }
}

impl Drop for HomeSessionServer {
    fn drop(&mut self) {
        let peer_id = self.context.peer_id();
        debug!("dropping session {}", peer_id);
        self.server.sessions.borrow_mut().remove(&peer_id);
    }
}

impl HomeSession for HomeSessionServer {
    fn update(&self, own_prof: OwnProfile) -> Box<Future<Item = (), Error = Error>> {
        if own_prof.id() != self.context.peer_id() {
            return Box::new(future::err(ErrorKind::ProfileMismatch.into()));
        }

        if own_prof.public_key() != self.context.peer_pubkey() {
            return Box::new(future::err(ErrorKind::PublicKeyMismatch.into()));
        }

        let upd_fut = self
            .server
            .hosted_profile_db
            .borrow()
            .get(&own_prof.id())
            // NOTE Block with "return" is needed, see https://stackoverflow.com/questions/50391668/running-asynchronous-mutable-operations-with-rust-futures
            .and_then({
                let distributed_store = self.server.public_profile_dht.clone();
                let pub_prof = own_prof.public_data();
                move |_own_prof_orig| {
                    // Update public profile parts in distributed storage (e.g. DHT)
                    return distributed_store
                        .borrow_mut()
                        .set_public(pub_prof);
                }
            })
            .and_then({
                let local_store = self.server.hosted_profile_db.clone();
                move |_| {
                    // Update private profile info in local storage only (e.g. SQL)
                    return local_store
                        .borrow_mut()
                        .set(own_prof);
                }
            })
            // TODO: fix it after storage error refactorings
            .map_err(|e| ErrorKind::ProfileUpdateFailed.into());

        Box::new(upd_fut)
    }

    // TODO is the ID of the new home enough here or do we need the whole profile?
    // TODO newhome should be stored and some special redirect to new home should be sent when someone looking for the profile
    fn unregister(&self, _newhome: Option<Profile>) -> Box<Future<Item = (), Error = Error>> {
        let profile_id = self.context.peer_id().to_owned();
        let profile_key = self.context.peer_pubkey();

        // TODO is it the caller's responsibility to remove this home from the persona facet's homelist
        //      or should we do it here and save the results into the distributed public db?
        // TODO how to delete profile from self.server.hosted_profiles_db? We'll probably need a remove operation

        // Drop session reference from server
        self.server.sessions.borrow_mut().remove(&profile_id);

        // TODO force close/drop session connection after successful unregister().
        //      Ideally self would be consumed here, but that'd require binding to self: Box<Self> or Rc<Self> to compile within a trait.

        let local_fut = self.server.hosted_profile_db.borrow_mut().clear(&profile_key);
        let unreg_fut = self
            .server
            .public_profile_dht
            .borrow_mut()
            .clear_public_local(&profile_key)
            .and_then(|_| local_fut)
            .map_err(|e| e.context(ErrorKind::UnregisterFailed).into());

        Box::new(unreg_fut)
    }

    // TODO add argument in a later milestone, presence: Option<AppMessageFrame>) ->
    fn checkin_app(&self, app: &ApplicationId) -> AsyncStream<Box<IncomingCall>, String> {
        let (sender, receiver) = mpsc::channel(CHANNEL_CAPACITY);

        match self.apps.borrow_mut().insert(app.to_owned(), ServerSink::Sender(sender.clone())) {
            Some(ServerSink::Sender(old_sender)) => {
                // NOTE consuming the calls stream multiple times is likely a client implementation error
                self.server.handle.spawn(
                    old_sender.send( Err( "WARNING: Repeated call of HomeSession::checkin_app() detected, this channel is dropped, using the new one".to_owned() ) )
                        .map( |_sender| () )
                        .map_err( |_e| () )
                )
            }
            Some(ServerSink::Buffer(call_vec)) => {
                // Send all collected calls from buffer as we now finally have a channel to the app
                // TODO use persistent storage for calls when profile is offline and delegate them here
                self.server.handle.spawn(
                    sender.send_all(stream::iter_ok(call_vec)).map(|_sender| ()).map_err(|_e| ()),
                )
            }
            None => {}
        }

        // TODO how to detect dropped stream and remove the sink from the session?
        receiver
    }

    // TODO investigate if race condition is possible, e.g. an event was sent out to the old_sender,
    //      and a repeated events() call is received. In this case, can we be sure that the event
    //      has been processed via the old_sender?
    fn events(&self) -> AsyncStream<ProfileEvent, String> {
        let (sender, receiver) = mpsc::channel(CHANNEL_CAPACITY);

        // Set up events with the new channel and check the old event sink
        match self.events.replace(ServerSink::Sender(sender.clone())) {
            // We already had another channel properly set up
            ServerSink::Sender(old_sender) => {
                // NOTE consuming the events stream multiple times is likely a client implementation error
                self.server.handle.spawn(
                    old_sender.send( Err( "WARNING: Repeated call of HomeSession::events() detected, this channel is dropped, using the new one".to_owned() ) )
                        .map( |_sender| () )
                        .map_err( |_e| () )
                )
            }
            // The client was not listening to events so far, the channel is brand new
            ServerSink::Buffer(msg_vec) => {
                // Send all collected messages from buffer as we now finally have a channel to the user
                // TODO use persistent storage for events when profile is offline and delegate them here
                self.server.handle.spawn(
                    sender.send_all(stream::iter_ok(msg_vec)).map(|_sender| ()).map_err(|_e| ()),
                )
            }
        }

        receiver
    }

    // TODO consider removing this after testing
    fn ping(&self, txt: &str) -> Box<Future<Item = String, Error = Error>> {
        debug!("Ping received `{}`, sending it back", txt);
        Box::new(future::ok(txt.to_owned()))
    }
}
