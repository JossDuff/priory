use libp2p::{core::multiaddr::Multiaddr, gossipsub::TopicHash};

pub enum Command {
    GossipsubPublish { topic: TopicHash, data: Vec<u8> },
    AddExternalAddress { multiaddr: Multiaddr },
}
