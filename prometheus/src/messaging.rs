use claims::model::*;
use mercury_connect::service::ConnectService;
use mercury_connect::*;
//use mercury_home_protocol::AsyncFallible;

use crate::api::*;
use crate::*;

pub struct MessagingImpl {
    //    connect_service: ConnectService,
}

impl MessagingImpl {
    pub fn new() -> Self {
        //        let connect_service = ConnectService::new();
        Self {}
    }
}

impl MessageApi for MessagingImpl {
    fn send_message(&self, _to: &ProfileId, _message: &MessageContent) -> AsyncFallible<()> {
        // - load to_profile from public DHT-liek storage
        // - connect home of that profile
        // - send message via home to target profile
        unimplemented!()
    }

    fn list_messages(&self, _with: &ProfileId) -> AsyncFallible<Vec<Message>> {
        unimplemented!()
    }
}
