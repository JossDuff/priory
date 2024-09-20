use crate::MyBehaviour;
use anyhow::Result;
use libp2p::{
    core::multiaddr::Multiaddr,
    gossipsub::TopicHash,
    swarm::{Swarm, SwarmEvent},
    PeerId,
};

pub enum Command {
    SetSpecialHandler {
        special_handler: Box<dyn FnMut(&SwarmEvent<MyBehaviour>) -> bool>,
    },
    // Gossipsub commands
    GossipsubPublish {
        topic: TopicHash,
        data: Vec<u8>,
    },
    GossipsubAddExplicitPeer {
        peer_id: PeerId,
    },
    GossipsubRemoveExplicitPeer {
        peer_id: PeerId,
    },
    // Swarm commands
    AddExternalAddress {
        multiaddr: Multiaddr,
    },
    Dial {
        multiaddr: Multiaddr,
    },
    ListenOn {
        multiaddr: Multiaddr,
    },
    // Kademlia commands
    KademliaAddAddress {
        peer_id: PeerId,
        multiaddr: Multiaddr,
    },
    KademliaRemoveAddress {
        peer_id: PeerId,
        multiaddr: Multiaddr,
    },
    // toggle relay commands
    IsRelayEnabled,
}

pub fn handle_command(swarm: &mut Swarm<MyBehaviour>, command: &Command) -> Result<()> {
    todo!()
}
