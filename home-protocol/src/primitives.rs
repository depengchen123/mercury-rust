use serde::{de::Error as DeSerError, ser::SerializeSeq};
use serde::{Deserialize as DeSer, Deserializer, Serializer};
use serde_derive::{Deserialize, Serialize};

use bincode::serialize;
use failure::Fallible;
use multiaddr::{Multiaddr, ToMultiaddr};

use crate::*;

pub use claims::model::{AttributeId, AttributeMap, AttributeValue, ProfileId, Signature, Version};
pub use did::model::{PrivateKey, PublicKey};

pub type Profile = claims::model::PublicProfileData;
pub type OwnProfile = claims::model::PrivateProfileData;

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, PartialOrd, Serialize)]
pub struct ApplicationId(pub String);

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostedFacet {
    /// `homes` contain items with `relation_type` "home", with proofs included.
    /// Current implementation supports only a single home stored in `homes[0]`,
    /// Support for multiple homes will be implemented in a future release.
    pub homes: Vec<RelationProof>,
    pub data: Vec<u8>,
}

impl HostedFacet {
    const ATTRIBUTE_ID: &'static str = "osg_hosted";

    pub fn new(homes: Vec<RelationProof>, data: Vec<u8>) -> Self {
        Self { homes, data }
    }

    fn set_attribute(&self, attributes: &mut AttributeMap) {
        let facet_str = serde_json::to_string(&self)
            .expect("This can fail only with failing custom Serialize() or having non-string keys");
        attributes.insert(Self::ATTRIBUTE_ID.to_string(), facet_str);
    }

    pub fn to_attribute_map(&self) -> AttributeMap {
        let mut attributes = AttributeMap::new();
        self.set_attribute(&mut attributes);
        attributes
    }

    fn to_hosted(attributes: &AttributeMap) -> Option<Self> {
        attributes
            .get(Self::ATTRIBUTE_ID)
            .and_then(|facet_str| serde_json::from_str(facet_str).ok())
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct HomeFacet {
    /// Addresses of the same home server. A typical scenario of multiple addresses is when there is
    /// one IPv4 address/port, one onion address/port and some IPv6 address/port pairs.
    #[serde(serialize_with = "serialize_multiaddr_vec")]
    #[serde(deserialize_with = "deserialize_multiaddr_vec")]
    pub addrs: Vec<Multiaddr>,
    pub data: Vec<u8>,
}

impl HomeFacet {
    const ATTRIBUTE_ID: &'static str = "osg_home_addresses";

    pub fn new(addrs: Vec<Multiaddr>, data: Vec<u8>) -> Self {
        Self { addrs, data }
    }

    fn set_attributes(&self, attributes: &mut AttributeMap) {
        let facet_str = serde_json::to_string(&self)
            .expect("This can fail only with failing custom Serialize() or having non-string keys");
        attributes.insert(Self::ATTRIBUTE_ID.to_string(), facet_str);
    }

    pub fn to_attribute_map(&self) -> AttributeMap {
        let mut attributes = AttributeMap::new();
        self.set_attributes(&mut attributes);
        attributes
    }

    fn to_home(attributes: &AttributeMap) -> Option<Self> {
        attributes
            .get(Self::ATTRIBUTE_ID)
            .and_then(|facet_str| serde_json::from_str(facet_str).ok())
    }
}

pub trait ProfileFacets {
    fn to_home(&self) -> Option<HomeFacet>;
    fn set_home(&mut self, home: &HomeFacet);

    fn to_hosted(&self) -> Option<HostedFacet>;
    fn set_hosted(&mut self, hosted: &HostedFacet);
    fn is_hosted_on(&self, home_id: &ProfileId) -> bool;
    fn add_hosted_on(&mut self, host_proof: &RelationProof) -> Result<(), Error>;
}

impl ProfileFacets for Profile {
    fn to_home(&self) -> Option<HomeFacet> {
        HomeFacet::to_home(self.attributes())
    }
    fn set_home(&mut self, home: &HomeFacet) {
        home.set_attributes(self.mut_attributes())
    }

    fn to_hosted(&self) -> Option<HostedFacet> {
        HostedFacet::to_hosted(self.attributes())
    }
    fn set_hosted(&mut self, hosted: &HostedFacet) {
        hosted.set_attribute(self.mut_attributes())
    }

    fn is_hosted_on(&self, home_id: &ProfileId) -> bool {
        if let Some(hosted) = self.to_hosted() {
            hosted.homes.iter().any(|host_proof| host_proof.peer_id(&self.id()) == Ok(home_id))
        } else {
            false
        }
    }

    fn add_hosted_on(&mut self, host_proof: &RelationProof) -> Result<(), Error> {
        let mut hosted = self.to_hosted().unwrap_or_default();
        if self.is_hosted_on(host_proof.peer_id(&self.id())?) {
            return Err(ErrorKind::AlreadyRegistered.into());
        }

        hosted.homes.push(host_proof.to_owned());
        self.set_hosted(&hosted);
        Ok(())
    }
}

// NOTE the binary blob to be signed is rust-specific: Strings are serialized to a u64 (size) and the encoded string itself.
// TODO consider if this is platform-agnostic enough, especially when combined with capnproto
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, PartialOrd, Serialize)]
pub struct RelationSignablePart {
    pub relation_type: String,
    pub signer_id: ProfileId,
    pub peer_id: ProfileId,
    // TODO is a nonce needed?
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RelationHalfProof {
    // TODO consider renaming this to payload as it might contain structured semantic information
    pub relation_type: String,
    pub signer_id: ProfileId,
    pub signer_pubkey: PublicKey,
    pub peer_id: ProfileId,
    pub signature: Signature,
    // TODO is a nonce needed?
}

impl RelationHalfProof {
    pub fn new(relation_type: &str, peer_id: &ProfileId, signer: &dyn Signer) -> Fallible<Self> {
        let signable = RelationSignablePart::new(relation_type, signer.profile_id(), peer_id);
        Ok(Self {
            relation_type: relation_type.to_owned(),
            signer_id: signer.profile_id().to_owned(),
            signer_pubkey: signer.public_key(),
            peer_id: peer_id.to_owned(),
            signature: signable.sign(signer)?,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RelationProof {
    pub relation_type: String, // TODO inline halfproof fields with macro, if possible at all
    pub a_id: ProfileId,
    pub a_pub_key: PublicKey,
    pub a_signature: Signature,
    pub b_id: ProfileId,
    pub b_pub_key: PublicKey,
    pub b_signature: Signature,
    // TODO is a nonce needed?
}

// TODO should we ignore this in early stages?
/// This invitation allows a persona to register on the specified home.
//#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, PartialOrd, Serialize)]
//pub struct HomeInvitation {
//    pub home_id: ProfileId,
//
//    /// A unique string that identifies the invitation
//    pub voucher: String,
//
//    /// The signature of the home
//    pub signature: Signature,
//    // TODO is a nonce needed?
//    // TODO is an expiration time needed?
//}
//
//impl HomeInvitation {
//    pub fn new(home_id: &ProfileId, voucher: &str, signature: &Signature) -> Self {
//        Self {
//            home_id: home_id.to_owned(),
//            voucher: voucher.to_owned(),
//            signature: signature.to_owned(),
//        }
//    }
//}

pub fn serialize_multiaddr_vec<S>(x: &Vec<Multiaddr>, s: S) -> std::result::Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut seq = s.serialize_seq(Some(x.len()))?;
    for mr in x {
        match seq.serialize_element(&mr.to_string()) {
            Ok(_) => {
                ();
            }
            Err(e) => {
                return Err(e);
            }
        }
    }
    seq.end()
}

pub fn deserialize_multiaddr_vec<'de, D>(
    deserializer: D,
) -> std::result::Result<Vec<Multiaddr>, D::Error>
where
    D: Deserializer<'de>,
{
    let mapped: Vec<String> = DeSer::deserialize(deserializer)?;
    let mut res = Vec::new();
    for str_ma in mapped.iter() {
        match str_ma.to_multiaddr() {
            Ok(multi) => {
                res.push(multi);
            }
            Err(e) => {
                return Err(D::Error::custom(e));
            }
        }
    }
    Ok(res)
}

// TODO use the same kind of ContentId/Hash as in mercury_claims::model::content_id()
impl RelationSignablePart {
    pub(crate) fn new(relation_type: &str, signer_id: &ProfileId, peer_id: &ProfileId) -> Self {
        Self {
            relation_type: relation_type.to_owned(),
            signer_id: signer_id.to_owned(),
            peer_id: peer_id.to_owned(),
        }
    }

    pub(crate) fn serialized(&self) -> Vec<u8> {
        // TODO unwrap() can fail here in some special cases: when there is a limit set and it's exceeded - or when .len() is
        //      not supported for the types to be serialized. Neither is possible here, so the unwrap will not fail.
        //      But still, to be on the safe side, this serialization shoule be swapped later with a call that cannot fail.
        // TODO consider using unwrap_or( Vec::new() ) instead
        serialize(self).unwrap()
    }

    fn sign(&self, signer: &dyn Signer) -> Fallible<Signature> {
        signer.sign(&self.serialized())
    }
}

impl<'a> From<&'a RelationHalfProof> for RelationSignablePart {
    fn from(src: &'a RelationHalfProof) -> Self {
        RelationSignablePart {
            relation_type: src.relation_type.clone(),
            signer_id: src.signer_id.clone(),
            peer_id: src.peer_id.clone(),
        }
    }
}

impl RelationProof {
    pub const RELATION_TYPE_HOSTED_ON_HOME: &'static str = "hosted_on_home";
    pub const RELATION_TYPE_ENABLE_CALLS_BETWEEN: &'static str = "enable_call_between";

    fn new(
        relation_type: &str,
        a_id: &ProfileId,
        a_pubkey: &PublicKey,
        a_signature: &Signature,
        b_id: &ProfileId,
        b_pubkey: &PublicKey,
        b_signature: &Signature,
    ) -> Self {
        if a_id < b_id {
            Self {
                relation_type: relation_type.to_owned(),
                a_id: a_id.to_owned(),
                a_pub_key: a_pubkey.to_owned(),
                a_signature: a_signature.to_owned(),
                b_id: b_id.to_owned(),
                b_pub_key: b_pubkey.to_owned(),
                b_signature: b_signature.to_owned(),
            }
        }
        // TODO decide on inverting relation_type if needed, e.g. `a_is_home_of_b` vs `b_is_home_of_a`
        else {
            Self {
                relation_type: relation_type.to_owned(),
                a_id: b_id.to_owned(),
                a_pub_key: b_pubkey.to_owned(),
                a_signature: b_signature.to_owned(),
                b_id: a_id.to_owned(),
                b_pub_key: a_pubkey.to_owned(),
                b_signature: a_signature.to_owned(),
            }
        }
    }

    pub fn sign_remaining_half(
        half_proof: &RelationHalfProof,
        signer: &dyn Signer,
    ) -> Result<Self, Error> {
        let my_profile_id = signer.profile_id();
        if half_proof.peer_id != *my_profile_id {
            Err(ErrorKind::RelationSigningFailed)?
        }

        let signable = RelationSignablePart::new(
            &half_proof.relation_type,
            my_profile_id,
            &half_proof.signer_id,
        );
        Ok(Self::new(
            &half_proof.relation_type,
            &half_proof.signer_id,
            &half_proof.signer_pubkey,
            &half_proof.signature,
            my_profile_id,
            &signer.public_key(),
            &signable
                .sign(signer)
                .map_err(|e| e.context(ErrorKind::RelationSigningFailed.into()))?,
        ))
    }

    // TODO relation-type should be more sophisticated once we have a proper metainfo schema there
    pub fn accessible_by(&self, app: &ApplicationId) -> bool {
        self.relation_type == app.0
    }

    pub fn peer_id(&self, my_id: &ProfileId) -> Result<&ProfileId, Error> {
        if self.a_id == *my_id {
            return Ok(&self.b_id);
        }
        if self.b_id == *my_id {
            return Ok(&self.a_id);
        }
        Err(ErrorKind::PeerIdRetreivalFailed)?
    }

    pub fn peer_signature(&self, my_id: &ProfileId) -> Result<&Signature, Error> {
        if self.a_id == *my_id {
            return Ok(&self.b_signature);
        }
        if self.b_id == *my_id {
            return Ok(&self.a_signature);
        }
        Err(ErrorKind::PeerIdRetreivalFailed)?
    }
}
