use std::cell::RefCell;
use std::net::{SocketAddr, TcpStream};
use std::rc::Rc;
use std::time::Duration;

use failure::{ensure, err_msg, Fallible};
use log::*;

use crate::client::{FallibleExtension, MsgPackRpc, RpcProfile, RpcPtr};
use crate::messages;
use osg::model::*;
use osg::profile::ProfilePtr;
use osg::repo::ProfileRepository;

pub struct RpcProfileRepository {
    address: SocketAddr,
    network_timeout: Duration,
    rpc: RefCell<Option<RpcPtr<TcpStream, TcpStream>>>,
}

impl RpcProfileRepository {
    pub fn new(address: &SocketAddr, network_timeout: Duration) -> Fallible<Self> {
        Ok(Self {
            address: *address,
            network_timeout,
            rpc: RefCell::new(Option::None),
        })
    }

    pub fn connect(
        address: &SocketAddr,
        network_timeout: Duration,
    ) -> Fallible<MsgPackRpc<TcpStream, TcpStream>> {
        debug!("Connecting to storage backend server {:?}", address);

        let tcp_stream = TcpStream::connect_timeout(&address, network_timeout)?;
        tcp_stream.set_read_timeout(Some(network_timeout))?;
        tcp_stream.set_write_timeout(Some(network_timeout))?;
        let tcp_stream_clone = tcp_stream.try_clone()?;
        let rpc = MsgPackRpc::new(tcp_stream, tcp_stream_clone);
        Ok(rpc)
    }

    fn rpc(&self) -> Fallible<RpcPtr<TcpStream, TcpStream>> {
        // TODO is really a lazy singleton init needed here? It makes types and
        //      everything much more complex, would be simpler in constructor
        if self.rpc.borrow().is_none() {
            let rpc = Self::connect(&self.address, self.network_timeout)?;
            *self.rpc.borrow_mut() = Option::Some(Rc::new(RefCell::new(rpc)));
        }

        Ok(self.rpc.borrow().clone().unwrap())
    }

    pub fn list_nodes(&self) -> Fallible<Vec<ProfileId>> {
        let params = messages::ListNodesParams {};
        let rpc = self.rpc()?;
        let response = rpc.borrow_mut().send_request("list_nodes", params)?;
        let node_vals = response
            .reply
            .ok_or_else(|| err_msg("Server returned no reply content for query"))?;
        let nodes = rmpv::ext::from_value(node_vals)?;
        Ok(nodes)
    }

    /// https://gitlab.libertaria.community/iop-stack/communication/morpheus-storage-daemon/wikis/Morpheus-storage-protocol#show-profile
    pub fn get_node(&self, id: &ProfileId) -> Fallible<ProfilePtr> {
        self.rpc().and_then(|rpc| {
            let rpc_profile = RpcProfile::new(id, rpc.clone());
            Ok(Rc::new(RefCell::new(rpc_profile)) as ProfilePtr)
        })
    }

    pub fn remove_node(&self, id: &ProfileId) -> Fallible<()> {
        // TODO remove this commented section if really not needed
        //        let profile_ptr = self.get_node(id)?;
        //        let mut profile = profile_ptr.borrow_mut();
        //
        //        let links = profile.links()?;
        //        for link in links {
        //            profile.remove_link(&link.peer_profile)?;
        //        }
        //        let attributes = profile.attributes()?;
        //        for attr in attributes.keys() {
        //            profile.clear_attribute(&attr)?;
        //        }

        self.rpc().and_then(|rpc| {
            let params = messages::RemoveNodeParams { id: id.clone() };
            rpc.borrow_mut().send_request("remove_node", params)?;
            Ok(())
        })
    }
}

impl ProfileRepository for RpcProfileRepository {
    /// https://gitlab.libertaria.community/iop-stack/communication/morpheus-storage-daemon/wikis/Morpheus-storage-protocol#show-profile
    fn get(&self, id: &ProfileId) -> Fallible<ProfileData> {
        let rpc_profile = self.get_node(id)?;
        ProfileData::try_from(rpc_profile)
    }

    /// https://gitlab.libertaria.community/iop-stack/communication/morpheus-storage-daemon/wikis/Morpheus-storage-protocol#create-profile
    fn set(&mut self, id: ProfileId, profile: ProfileData) -> Fallible<()> {
        ensure!(
            id == profile.id,
            "Implementation error: RpcProfileRepository got conlicting key and value: {} vs {}",
            id,
            profile.id
        );
        self.rpc().and_then(|rpc| {
            // TODO properly implement insert_or_update with the RPC
            let request = messages::AddNodeParams { id: id.clone() };
            rpc.borrow_mut()
                .send_request("add_node", request)
                .map(|_r| ())
                .key_not_existed_or_else(|| Ok(()))?;

            let rpc_clone = rpc.clone();
            let rpc_profile = RpcProfile::new(&id, rpc_clone);
            // TODO this shouldn't belong here, querying an empty attribute set shouldn't be an error
            rpc_profile.set_osg_attribute_map(AttributeMap::default())?;
            Ok(())
        })?;
        Ok(())
    }

    fn clear(&mut self, id: &ProfileId) -> Fallible<()> {
        // TODO set() should implement insert_or_update, so remove() should be removed
        self.remove_node(id)?;
        self.set(id.to_owned(), ProfileData::empty(id))

        // TODO remove commented section if not needed
        //        let params = messages::RemoveNodeParams { id: id.clone() };
        //        rpc.borrow_mut()
        //            .send_request("remove_node", request)
        //            .map(|_r| ())?;
        //        rpc.borrow_mut().
        //
        //        let mut rpc_profile = self.get(id)?;
        //
        //        self.rpc().and_then(|rpc| {
        //            let rpc_profile = RpcProfile::new(id, rpc);
        //            for link in profile.links {
        //                rpc_profile.remove_link(&link.peer_profile)?;
        //            }
        //            for attr in profile.attributes.keys() {
        //                rpc_profile.clear_attribute(&attr)?;
        //            }
        //        })?;
        //        Ok(())
    }

    fn followers(&self, id: &ProfileId) -> Fallible<Vec<Link>> {
        self.rpc().and_then(|rpc| {
            let params = messages::ListInEdgesParams { id: id.clone() };
            let response = rpc.borrow_mut().send_request("list_inedges", params)?;
            let reply_val = response
                .reply
                .ok_or_else(|| err_msg("Server returned no reply content for query"))?;
            let reply: messages::ListInEdgesReply = rmpv::ext::from_value(reply_val)?;
            let followers = reply
                .into_iter()
                .map(|peer_profile| Link { peer_profile })
                .collect();
            Ok(followers)
        })
    }
}
