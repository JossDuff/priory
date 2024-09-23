use crate::{MyBehaviour, P2pNode};
use anyhow::Result;
use libp2p::{
    core::multiaddr::Multiaddr,
    gossipsub::{IdentTopic, TopicHash},
    swarm::{Swarm, SwarmEvent},
    PeerId,
};
use tokio::sync::oneshot;

pub enum Command {
    // SetSpecialHandler {
    //     special_handler: Box<dyn FnMut(&SwarmEvent<MyBehaviour>) -> bool>,
    // },
    // Gossipsub commands
    GossipsubPublish {
        topic: TopicHash,
        data: Vec<u8>,
    },
    GossipsubAddExplicitPeer {
        peer_id: PeerId,
    },
    GossipsubSubscribe {
        topic: IdentTopic,
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
    KademliaBootstrap,
    // toggle relay commands
    IsRelayEnabled {
        sender: oneshot::Sender<bool>,
    },
    BootstrapP2pNetwork,
}

pub fn handle_command(p2p_node: &mut P2pNode, command: Command) -> Result<()> {
    let swarm = &mut p2p_node.swarm;
    let _ = match command {
        // Gossipsub commands
        Command::GossipsubPublish { topic, data } => {
            swarm
                .behaviour_mut()
                .gossipsub
                .publish(topic, data)
                .unwrap();
        }
        Command::GossipsubAddExplicitPeer { peer_id } => {
            swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
        }
        Command::GossipsubSubscribe { topic } => {
            swarm.behaviour_mut().gossipsub.subscribe(&topic).unwrap();
        }
        Command::GossipsubRemoveExplicitPeer { peer_id } => {
            swarm
                .behaviour_mut()
                .gossipsub
                .remove_explicit_peer(&peer_id);
        }
        // Swarm commands
        Command::AddExternalAddress { multiaddr } => {
            swarm.add_external_address(multiaddr);
        }
        Command::Dial { multiaddr } => {
            swarm.dial(multiaddr).unwrap();
        }
        Command::ListenOn { multiaddr } => {
            swarm.listen_on(multiaddr).unwrap();
        }
        // Kademlia commands
        Command::KademliaAddAddress { peer_id, multiaddr } => {
            swarm
                .behaviour_mut()
                .kademlia
                .add_address(&peer_id, multiaddr);
        }
        Command::KademliaBootstrap => {
            swarm.behaviour_mut().kademlia.bootstrap().unwrap();
        }
        // toggle relay commands
        Command::IsRelayEnabled { sender } => {
            sender
                .send(swarm.behaviour().toggle_relay.is_enabled())
                .unwrap();
        }
        // Update the status of bootsrapping
        Command::UpdateBootstrappingStatus { is_bootstrapping } => {
            p2p_node.is_bootstrapping = is_bootstrapping;
        }
    };

    Ok(())
}
