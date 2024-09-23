use crate::P2pNode;
use anyhow::Result;
use libp2p::{
    core::multiaddr::Multiaddr,
    gossipsub::{IdentTopic, TopicHash},
};

pub enum Command {
    // Gossipsub commands
    GossipsubPublish { topic: TopicHash, data: Vec<u8> },
    GossipsubSubscribe { topic: IdentTopic },
    // Swarm commands
    Dial { multiaddr: Multiaddr },
    ListenOn { multiaddr: Multiaddr },
    // Kademlia commands
    KademliaBootstrap,
}

pub fn handle_command(p2p_node: &mut P2pNode, command: Command) -> Result<()> {
    let swarm = &mut p2p_node.swarm;
    match command {
        // Gossipsub commands
        Command::GossipsubPublish { topic, data } => {
            swarm
                .behaviour_mut()
                .gossipsub
                .publish(topic, data)
                .unwrap();
        }
        Command::GossipsubSubscribe { topic } => {
            swarm.behaviour_mut().gossipsub.subscribe(&topic).unwrap();
        }
        // Swarm commands
        Command::Dial { multiaddr } => {
            swarm.dial(multiaddr).unwrap();
        }
        Command::ListenOn { multiaddr } => {
            swarm.listen_on(multiaddr).unwrap();
        }
        // Kademlia commands
        Command::KademliaBootstrap => {
            swarm.behaviour_mut().kademlia.bootstrap().unwrap();
        }
    };

    Ok(())
}
