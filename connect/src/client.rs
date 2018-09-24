use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::fmt::Display;

use failure::{Context, Fail, Backtrace};
use futures::{future, Future, IntoFuture};

use mercury_home_protocol::*;
use ::Relation;



#[derive(Debug)]
pub struct Error {
    inner: Context<ErrorKind>
}

#[derive(Copy, Clone, Eq, PartialEq, Debug, Fail)]
pub enum ErrorKind {
    #[fail(display= "connection to home failed")]
    ConnectionToHomeFailed,

    #[fail(display="handshake failed")]
    HandshakeFailed,

    #[fail(display= "peer id retreival failed")]
    PeerIdRetreivalFailed,

    #[fail(display= "failed to get contacts")]
    FailedToGetContacts,

    #[fail(display="failed to get session")]
    FailedToGetSession,

    #[fail(display="address conversion failed")]
    AddressConversionFailed,

    #[fail(display="failed to connect tcp stream")]
    ConnectionFailed,

    #[fail(display="failed to load profile")]
    FailedToLoadProfile,

    #[fail(display="failed to resolve profile")]
    FailedToResolveProfile,

    #[fail(display="home profile expected")]
    HomeProfileExpected,

    #[fail(display="failed to claim profile")]
    FailedToClaimProfile,

    #[fail(display="registration failed")]
    RegistrationFailed,

    #[fail(display="deregistration failed")]
    DeregistrationFailed,

    #[fail(display="pair request failed")]
    PairRequestFailed,

    #[fail(display="peer response failed")]
    PeerResponseFailed,

    #[fail(display="profile update failed")]
    ProfileUpdateFailed,

    #[fail(display="call failed")]
    CallFailed,

    #[fail(display="call refused")]
    CallRefused,

    #[fail(display="lookup failed")]
    LookupFailed,

    #[fail(display="no proof found for home")]
    HomeProofNotFound,

    #[fail(display="persona profile expected")]
    PersonaProfileExpected,

    #[fail(display="no homes found")]
    NoHomesFound,

    #[fail(display="login failed")]
    LoginFailed,

    #[fail(display="failed to get peer id")]
    FailedToGetPeerId,

    #[fail(display="unknown")]
    Unknown,
}

impl PartialEq for Error {
    fn eq(&self, other: &Error) -> bool {
        self.inner.get_context() == other.inner.get_context()
    }
}

impl Fail for Error {
    fn cause(&self) -> Option<&Fail> {
        self.inner.cause()
    }

    fn backtrace(&self) -> Option<&Backtrace> {
        self.inner.backtrace()
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        Display::fmt(&self.inner, f)
    }
}

impl Error {
    pub fn kind(&self) -> ErrorKind {
        *self.inner.get_context()
    }
}

impl From<ErrorKind> for Error {
    fn from(kind: ErrorKind) -> Error {
        Error { inner: Context::new(kind) }
    }
}

impl From<Context<ErrorKind>> for Error {
    fn from(inner: Context<ErrorKind>) -> Error {
        Error { inner: inner }
    }
}



pub trait HomeConnector
{
    /// Initiate a permanent connection to the home server defined by `home_profile`, or return an
    /// existing, live `Home` immediately.
    /// `home_profile` must have a HomeFacet with at least an address filled in.
    /// `signer` belongs to me.
    fn connect(&self, home_profile: &Profile, signer: Rc<Signer>) ->
        Box< Future<Item=Rc<Home>, Error=Error> >;
}



pub trait ProfileGateway
{
    fn signer(&self) -> &Signer;
    fn relations(&self) -> Box< Future<Item=Vec<Relation>, Error=Error> >;

    fn claim(&self, home: ProfileId, profile: ProfileId) ->
        Box< Future<Item=OwnProfile, Error=Error> >;

    /// `invite` is needed only if the home has a restrictive registration policy.
    fn register(&self, home: ProfileId, own_prof: OwnProfile, invite: Option<HomeInvitation>) ->
        Box< Future<Item=OwnProfile, Error=(OwnProfile,Error)> >;

    fn update(&self, home: ProfileId, own_prof: &OwnProfile) ->
        Box< Future<Item=(), Error=Error> >;

    // NOTE newhome is a profile that contains at least one HomeSchema different than this home
    fn unregister(&self, home: ProfileId, newhome: Option<Profile>) ->
        Box< Future<Item=(), Error=Error> >;


    fn pair_request(&self, relation_type: &str, with_profile_id: &ProfileId, pairing_url: Option<&str>) ->
        Box< Future<Item=(), Error=Error> >;

    fn pair_response(&self, proof: RelationProof) ->
        Box< Future<Item=(), Error=Error> >;

    fn call(&self, rel: RelationProof, app: ApplicationId, init_payload: AppMessageFrame,
            to_caller: Option<AppMsgSink>) ->
        Box< Future<Item=Option<AppMsgSink>, Error=Error> >;


    fn login(&self) ->
        Box< Future<Item=Rc<HomeSession>, Error=Error> >;
}



#[derive(Clone)]
pub struct ProfileGatewayImpl
{
    signer:         Rc<Signer>,
    profile_repo:   Rc<ProfileRepo>,
    home_connector: Rc<HomeConnector>,
    session_cache:  Rc<RefCell< HashMap<ProfileId, Rc<HomeSession>> >>,
}


impl ProfileGatewayImpl
{
    pub fn new(signer: Rc<Signer>, profile_repo: Rc<ProfileRepo>, home_connector: Rc<HomeConnector>)
        -> Self { Self{ signer, profile_repo, home_connector, session_cache: Default::default() } }

    pub fn connect_home(&self, home_profile_id: &ProfileId)
        -> Box< Future<Item=Rc<Home>, Error=Error> >
    {
        Self::connect_home2( home_profile_id, self.profile_repo.clone(),
                             self.home_connector.clone(), self.signer.clone() )
    }

    fn connect_home2(home_profile_id: &ProfileId, prof_repo: Rc<ProfileRepo>,
                     connector: Rc<HomeConnector>, signer: Rc<Signer>)
        -> Box< Future<Item=Rc<Home>, Error=Error> >
    {
        let home_conn_fut = prof_repo.load(home_profile_id)
            .inspect( move |home_profile| debug!("Finished loading details for home {}", home_profile.id) )
            .map_err(|err| err.context(ErrorKind::FailedToLoadProfile).into())
            .and_then( move |home_profile| connector.connect(&home_profile, signer) );

        Box::new(home_conn_fut)
    }


    pub fn login_home(&self, home_profile_id: ProfileId) ->
        Box< Future<Item=Rc<HomeSession>, Error=Error> >
    {
        if let Some(ref session_rc) = self.session_cache.borrow().get(&home_profile_id)
            { return Box::new( Ok( Rc::clone(session_rc) ).into_future() ) }

        let home_id = home_profile_id.clone();
        let my_profile_id = self.signer.profile_id().to_owned();
        let session_cache = self.session_cache.clone();
        let login_fut = self.profile_repo.load(&my_profile_id)
            .map_err(|err| err.context(ErrorKind::FailedToLoadProfile).into())
            .and_then( |profile|
            {
                match profile.facet
                {
                    ProfileFacet::Persona(persona) => persona.homes.iter()
                        .filter(move |home_proof|
                            home_proof.peer_id(&my_profile_id)
                                .and_then(|peer_id|
                                    if *peer_id == home_id { Ok(true) }
                                    else { Err(::mercury_home_protocol::error::ErrorKind::PeerIdRetreivalFailed.into()) }
                                )
                                .is_ok()
                        )
                        .map( |home_proof| home_proof.to_owned() )
                        .nth(0)
                        .ok_or(ErrorKind::HomeProofNotFound.into()),

                    _ => Err(ErrorKind::PersonaProfileExpected.into())
                }
            } )
            .and_then(
            {
                let profile_repo_clone = self.profile_repo.clone();
                let home_connector_clone = self.home_connector.clone();
                let signer_clone = self.signer.clone();
                move |home_proof| {
                    Self::connect_home2(&home_profile_id, profile_repo_clone, home_connector_clone, signer_clone)
                        .and_then( move |home| {
                            home.login(&home_proof)
                                .map_err( |err| err.context(ErrorKind::LoginFailed).into() )
                                .inspect( move |session| {
                                    session_cache.borrow_mut().insert( home_profile_id.to_owned(), session.clone() );
                                } )
                        } )
                }
            });
        Box::new(login_fut)
    }


    pub fn any_home_of(&self, profile: &Profile) ->
        Box< Future<Item=(RelationProof, Rc<Home>), Error=Error> >
    {
        ProfileGatewayImpl::any_home_of2( profile, self.profile_repo.clone(),
                                          self.home_connector.clone(), self.signer.clone() )
    }


    fn any_home_of2(profile: &Profile, prof_repo: Rc<ProfileRepo>,
                    connector: Rc<HomeConnector>, signer: Rc<Signer>) ->
        Box< Future<Item=(RelationProof, Rc<Home>), Error=Error> >
    {
        let homes = match profile.facet {
            // TODO consider how to get homes/addresses for apps and smartfridges
            ProfileFacet::Persona(ref facet) => facet.homes.clone(),
            _ => return Box::new(future::err(ErrorKind::HomeProfileExpected.into())),
        };

        let home_conn_futs = homes.iter()
            .map( move |home_proof|
            {
                let prof_repo = prof_repo.clone();
                let connector = connector.clone();
                let proof = home_proof.to_owned();
                match home_proof.peer_id(&profile.id) {
                    Ok(ref home_id) => {
                        debug!("Scheduling connect_home2 for home id {}", home_id);
                        let conn_fut = Self::connect_home2( home_id.to_owned(), prof_repo, connector, signer.clone() )
                            .map( move |home| (proof, home) );
                        Box::new(conn_fut) as Box<Future<Item=_, Error=Error>>
                    },
                    Err(e) => Box::new( future::err(e.context(ErrorKind::FailedToGetPeerId).into()) ),
                }
            } )
            .collect::<Vec<_>>();

        // NOTE needed because select_ok() panics for empty lists instead of simply returning an error
        if home_conn_futs.len() == 0
            { return Box::new( future::err(ErrorKind::NoHomesFound.into())) }

        // Pick first successful home connection
        debug!("Try connecting to {} homes", home_conn_futs.len());
        let result = future::select_ok(home_conn_futs)
            .map( |(home_conn, _pending_conn_futs)| home_conn )
            .inspect( |_home_conn| debug!("Connected to first home, ignoring the rest") );
        Box::new(result)
    }
}


impl ProfileGateway for ProfileGatewayImpl
{
    fn signer(&self) -> &Signer { &*self.signer }


    fn relations(&self) -> Box< Future<Item=Vec<Relation>, Error=Error> >
    {
        unimplemented!()
    }


    fn claim(&self, home_id: ProfileId, profile: ProfileId) ->
        Box< Future<Item=OwnProfile, Error=Error> >
    {
        let claim_fut = self.connect_home(&home_id)
            .map_err(|err| err.context(ErrorKind::ConnectionToHomeFailed).into())
            .and_then( move |home| {
                home
                    .claim(profile)
                    .map_err(|err| err.context(ErrorKind::FailedToClaimProfile).into())

            });
        Box::new(claim_fut)
    }


    fn register(&self, home_id: ProfileId, own_prof: OwnProfile,
                invite: Option<HomeInvitation>) ->
        Box< Future<Item=OwnProfile, Error=(OwnProfile,Error)> >
    {
        let own_prof_clone = own_prof.clone();
        let half_proof = RelationHalfProof::new(RelationProof::RELATION_TYPE_HOSTED_ON_HOME, &home_id, &*self.signer);
        let reg_fut = self.connect_home(&home_id)
            .map_err( move |e| (own_prof_clone, e) )
            .and_then( move |home| {
                home
                    .register(own_prof, half_proof, invite)
                    .map_err(|(own_prof, err)| (own_prof, err.context(ErrorKind::RegistrationFailed).into()))
            });
        Box::new(reg_fut)
    }


    fn update(&self, home_id: ProfileId, own_prof: &OwnProfile) ->
        Box< Future<Item=(), Error=Error> >
    {
        let own_profile_clone = own_prof.clone();
        let upd_fut = self.login_home(home_id)
            .map_err(|err| err.context(ErrorKind::LoginFailed).into())
            .and_then( move |session| {
                session
                    .update(own_profile_clone)
                    .map_err(|err| err.context(ErrorKind::ProfileUpdateFailed).into())
            });
        Box::new(upd_fut)
    }


    fn unregister(&self, home_id: ProfileId, newhome_id: Option<Profile>) ->
        Box< Future<Item=(), Error=Error> >
    {
        let unreg_fut = self.login_home(home_id)
            .map_err(|err| err.context(ErrorKind::LoginFailed).into())
            .and_then( move |session| {
                session
                    .unregister(newhome_id)
                    .map_err(|err| err.context(ErrorKind::DeregistrationFailed).into())
            });
        Box::new(unreg_fut)
    }


    fn pair_request(&self, relation_type: &str, with_profile_id: &ProfileId, _pairing_url: Option<&str>) ->
        Box< Future<Item=(), Error=Error> >
    {
        let profile_repo_clone = self.profile_repo.clone();
        let home_connector_clone = self.home_connector.clone();
        let signer_clone = self.signer.clone();
        let rel_type_clone = relation_type.to_owned();

//        let profile_fut = match pairing_url {
//            Some(url) => self.profile_repo.resolve(url),
//            None      => self.profile_repo.load(with_profile_id),
//        };

        let profile_fut = self.profile_repo.load(with_profile_id);

        let pair_fut = profile_fut
            .map_err(|err| err.context(ErrorKind::FailedToLoadProfile).into())
            .and_then( move |profile|
            {
                //let half_proof = ProfileGatewayImpl::new_half_proof(rel_type_clone.as_str(), &profile.id, signer_clone.clone() );
                let half_proof = RelationHalfProof::new(&rel_type_clone, &profile.id, &*signer_clone.clone() );
                ProfileGatewayImpl::any_home_of2(&profile, profile_repo_clone, home_connector_clone, signer_clone)
                    .and_then( move |(_home_proof, home)| {
                        home
                            .pair_request(half_proof)
                            .map_err(|err| err.context(ErrorKind::PairRequestFailed).into())
                    })
            } );

        Box::new(pair_fut)
    }


    fn pair_response(&self, proof: RelationProof) ->
        Box< Future<Item=(), Error=Error> >
    {
        let peer_id = match proof.peer_id( self.signer.profile_id() ) {
            Ok(peer_id) => peer_id.to_owned(),
            Err(e) => return Box::new( Err(e.context(ErrorKind::LookupFailed).into()).into_future() ),
        };

        let pair_fut = self.profile_repo.load(&peer_id)
            .map_err(|err| err.context(ErrorKind::FailedToLoadProfile).into())
            .and_then( {
                let profile_repo = self.profile_repo.clone();
                let connector = self.home_connector.clone();
                let signer = self.signer.clone();
                move |profile| Self::any_home_of2(&profile, profile_repo, connector, signer)
            } )
            .and_then( move |(_home_proof, home)| {
                home
                    .pair_response(proof)
                    .map_err(|err| err.context(ErrorKind::PeerResponseFailed).into())
            });
        Box::new(pair_fut)
    }


    fn call(&self, proof: RelationProof, app: ApplicationId, init_payload: AppMessageFrame,
            to_caller: Option<AppMsgSink>) ->
        Box< Future<Item=Option<AppMsgSink>, Error=Error> >
    {
        let peer_id = match proof.peer_id( self.signer.profile_id() ) {
            Ok(id) => id.to_owned(),
            Err(e) => return Box::new( Err(e.context(ErrorKind::LookupFailed).into()).into_future() ),
        };

        let profile_repo = self.profile_repo.clone();
        let home_connector = self.home_connector.clone();
        let signer = self.signer.clone();
        let call_fut = self.profile_repo.load(&peer_id)
            .map_err(|err| err.context(ErrorKind::FailedToLoadProfile).into())
            .and_then( |profile| Self::any_home_of2(&profile, profile_repo, home_connector, signer) )
            .and_then( move |(_home_proof, home)| {
                home.call(app, CallRequestDetails { relation: proof, init_payload: init_payload, to_caller: to_caller } )
                    .map_err(|err| err.context(ErrorKind::CallFailed).into())
            });
        Box::new(call_fut)
    }


    // TODO this should try connecting to ALL of our homes
    fn login(&self) -> Box< Future<Item=Rc<HomeSession>, Error=Error> >
    {
        if let Some(ref session_rc) = self.session_cache.borrow().values().next()
            { return Box::new( Ok( Rc::clone(session_rc) ).into_future() ) }

        let my_profile_id = self.signer.profile_id().to_owned();
        let session_cache = self.session_cache.clone();
        let log_fut = self.profile_repo.load( self.signer.profile_id() )
            .map_err( |err| err.context(ErrorKind::LoginFailed).into() )
            .and_then( {
                let profile_repo_clone = self.profile_repo.clone();
                let home_conn_clone = self.home_connector.clone();
                let signer_clone = self.signer.clone();
                move |profile| {
                    debug!("Client profile was loaded for login, connecting home");
                    ProfileGatewayImpl::any_home_of2(
                        &profile, profile_repo_clone, home_conn_clone, signer_clone)
                }
            } )
            .and_then( move |(home_proof, home)| {
                debug!("Home connection established, logging in");
                let home_id = match home_proof.peer_id(&my_profile_id) {
                    Ok(id) => id.to_owned(),
                    Err(e) => return Box::new( Err( e.context(ErrorKind::Unknown).into() ).into_future() ) as Box<Future<Item=_,Error=_>>,
                };
                let login_fut = home.login(&home_proof)
                    .map_err(|err| err.context(ErrorKind::LoginFailed).into())
                    .inspect( move |session| {
                        session_cache.borrow_mut().insert( home_id, session.clone() ); } );
                Box::new(login_fut)
            });

        Box::new(log_fut)
    }
}
