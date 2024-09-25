use anyhow::{Context, Result};
use futures::executor::block_on;
use libp2p::{
    core::{
        multiaddr::{Multiaddr, Protocol},
        PeerId,
    },
    gossipsub::{self, IdentTopic, Message, TopicHash},
    identify,
    swarm::SwarmEvent,
};
use std::{
    collections::{HashMap, HashSet},
    net::Ipv4Addr,
};
use tokio::{
    sync::{
        mpsc::{Receiver, Sender},
        oneshot,
    },
    time::{sleep, Duration},
};
use tracing::info;

use crate::config::Config;
use crate::p2p_node::{
    MyBehaviourEvent, P2pNode, Peer, I_HAVE_RELAYS_PREFIX, WANT_RELAY_FOR_PREFIX,
};

// These are the events that we need some information from during bootstrapping.
// When encountered in the main thread, the specified data is copied here and the
// event is also handled by the common handler.
pub enum BootstrapEvent {
    ConnectionEstablished { peer_id: PeerId },
    OutgoingConnectionError,
    GossipsubMessage { message: Message },
    IdentifySent,
    IdentifyReceived,
    DcutrConnectionSuccessful { remote_peer_id: PeerId },
    DcutrConnectionFailed { remote_peer_id: PeerId },
}

impl BootstrapEvent {
    pub fn try_from_swarm_event(event: &SwarmEvent<MyBehaviourEvent>) -> Option<BootstrapEvent> {
        match event {
            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                Some(BootstrapEvent::ConnectionEstablished { peer_id: *peer_id })
            }
            SwarmEvent::OutgoingConnectionError { .. } => {
                Some(BootstrapEvent::OutgoingConnectionError)
            }
            SwarmEvent::Behaviour(MyBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                message,
                ..
            })) => Some(BootstrapEvent::GossipsubMessage {
                message: message.clone(),
            }),
            SwarmEvent::Behaviour(MyBehaviourEvent::Identify(identify::Event::Sent { .. })) => {
                Some(BootstrapEvent::IdentifySent)
            }
            SwarmEvent::Behaviour(MyBehaviourEvent::Identify(identify::Event::Received {
                ..
            })) => Some(BootstrapEvent::IdentifyReceived),
            SwarmEvent::Behaviour(MyBehaviourEvent::Dcutr(event)) => {
                let remote_peer_id = event.remote_peer_id;
                match event.result {
                    Ok(_) => Some(BootstrapEvent::DcutrConnectionSuccessful { remote_peer_id }),
                    Err(_) => Some(BootstrapEvent::DcutrConnectionFailed { remote_peer_id }),
                }
            }
            _ => None,
        }
    }
}

pub enum BootstrapCommand {
    // Gossipsub commands
    GossipsubPublish {
        topic: TopicHash,
        data: Vec<u8>,
    },
    GossipsubSubscribe {
        topic: IdentTopic,
    },
    // Swarm commands
    Dial {
        multiaddr: Multiaddr,
    },
    ListenOn {
        multiaddr: Multiaddr,
    },
    // P2p node commands
    MyRelays {
        sender: oneshot::Sender<HashSet<Peer>>,
    },
}

pub fn handle_bootstrap_command(p2p_node: &mut P2pNode, command: BootstrapCommand) -> Result<()> {
    let swarm = &mut p2p_node.swarm;
    match command {
        // Gossipsub commands
        BootstrapCommand::GossipsubPublish { topic, data } => {
            swarm
                .behaviour_mut()
                .gossipsub
                .publish(topic, data)
                .unwrap();
        }
        BootstrapCommand::GossipsubSubscribe { topic } => {
            swarm.behaviour_mut().gossipsub.subscribe(&topic).unwrap();
        }
        // Swarm commands
        BootstrapCommand::Dial { multiaddr } => {
            swarm.dial(multiaddr).unwrap();
        }
        BootstrapCommand::ListenOn { multiaddr } => {
            swarm.listen_on(multiaddr).unwrap();
        }
        BootstrapCommand::MyRelays { sender } => {
            let my_relays = p2p_node.relays.clone();
            sender.send(my_relays).unwrap();
        }
    };

    Ok(())
}

pub async fn bootstrap_swarm(
    cfg: Config,
    command_sender: Sender<BootstrapCommand>,
    event_receiver: &mut Receiver<BootstrapEvent>,
    topic: gossipsub::IdentTopic,
) -> Result<()> {
    info!("BOOTSTRAPPING");
    // subscribes to our IdentTopic
    command_sender
        .send(BootstrapCommand::GossipsubSubscribe {
            topic: topic.clone(),
        })
        .await
        .unwrap();

    // Listen on all interfaces and the specified port
    let listen_addr_tcp = Multiaddr::empty()
        .with(Protocol::from(Ipv4Addr::UNSPECIFIED))
        .with(Protocol::Tcp(cfg.port));
    command_sender
        .send(BootstrapCommand::ListenOn {
            multiaddr: listen_addr_tcp,
        })
        .await
        .unwrap();

    let listen_addr_quic = Multiaddr::empty()
        .with(Protocol::from(Ipv4Addr::UNSPECIFIED))
        .with(Protocol::Udp(cfg.port))
        .with(Protocol::QuicV1);
    command_sender
        .send(BootstrapCommand::ListenOn {
            multiaddr: listen_addr_quic,
        })
        .await
        .unwrap();

    // Wait to listen on all interfaces.
    // Likely listening on all interfaces after a second.
    sleep(Duration::from_secs(1)).await;

    // keep track of the nodes that we'll later have to hole punch into
    let mut failed_to_dial: Vec<Peer> = Vec::new();

    // try to dial all peers in config
    for peer in &cfg.peers.clone() {
        let peer_multiaddr = &peer.multiaddr;

        // dial peer
        // if successful add to DHT
        // if failure wait until we've made contact with the dht and find a peer to holepunch
        command_sender
            .send(BootstrapCommand::Dial {
                multiaddr: peer_multiaddr.clone(),
            })
            .await
            .unwrap();

        // loop until we either connect or fail to connect
        loop {
            // TODO: could we get these events from other sources than the above dial?  Should we
            // be checking that the dialed multiaddr is the one we established connection with?
            match event_receiver
                .recv()
                .await
                .context("event sender shouldn't drop")
                .unwrap()
            {
                BootstrapEvent::ConnectionEstablished { peer_id, .. } => {
                    // have to make sure this event is about the node we just dialed
                    if peer_id == peer.peer_id {
                        break;
                    }
                }
                BootstrapEvent::OutgoingConnectionError { .. } => {
                    // TODO: make sure this is an error because the node is behind a firewall (I
                    // think Transport error?)
                    // TODO: have to make sure this event is about the node we just dialed (how???)
                    failed_to_dial.push(peer.clone());
                    break;
                }
                // ignore other events
                _ => (),
            }
        }
    }

    // unable to dial any of the nodes
    if failed_to_dial.len() == cfg.peers.len() && !cfg.peers.is_empty() {
        panic!("Couldn't connect to any adress listed as a peer in the config");
    }

    // When DHT is all set and nodes know you're a relay, try to find relays for those nodes you
    // failed to dial earlier
    // TODO: These should happen concurrently, each on its own tokio thread that handles all the
    // events.  Or maybe it has a channel that receives events and sends to one thread that handles
    // all events?? Hmmm
    for peer in failed_to_dial {
        let peer_id = peer.peer_id;

        let query = format!("{WANT_RELAY_FOR_PREFIX}{peer_id}");
        command_sender
            .send(BootstrapCommand::GossipsubPublish {
                topic: topic.clone().into(),
                data: query.into(),
            })
            .await
            .unwrap();

        // Wait until we hear a response from a relay claiming they know this peer_id (or timeout)
        let mut possible_relays: Vec<Multiaddr> = Vec::new();
        // TODO: add a timeout (in case nobody is connected to this node) and don't have relay_address be mutable
        loop {
            if let BootstrapEvent::GossipsubMessage { message, .. } = event_receiver
                .recv()
                .await
                .context("event sender shouldn't drop")
                .unwrap()
            {
                let message = String::from_utf8_lossy(&message.data);
                // should respond with {prefix}{target_peer_id} {relay_multiaddr}
                if let Some(str) = message.strip_prefix(I_HAVE_RELAYS_PREFIX) {
                    let str: Vec<&str> = str.split(" ").collect();
                    assert!(!str.is_empty(), "must return at least its own peer_id");

                    // peer doesn't have any relays or isn't willing to share
                    if str.len() == 1 {
                        break;
                    }

                    let target_peer_id: PeerId = str[0].parse().unwrap();

                    // if the message is about the peer we care about, break and try to dial that
                    // multiaddr
                    if target_peer_id == peer_id {
                        // add all the relays to the list
                        for multiaddr_str in str.iter().skip(1) {
                            possible_relays.push(multiaddr_str.parse().unwrap());
                        }
                        info!("Found relays for peer {}", peer_id);
                        break;
                    }
                }
            }
        }

        // first check if we already are connected to any of these relays
        let (sender, receiver) = oneshot::channel();
        command_sender
            .send(BootstrapCommand::MyRelays { sender })
            .await
            .unwrap();

        let my_relays = receiver.await.unwrap();
        let (common_relays, possible_relays) = compare_relay_lists(my_relays, possible_relays);

        for relay in common_relays {
            let relay_address_with_peer_id = relay.multiaddr.with_p2p(relay.peer_id).unwrap();
            // attempt to holepunch with one of the relays we know
            if attempt_holepunch(
                relay_address_with_peer_id.clone(),
                peer_id,
                command_sender.clone(),
                event_receiver,
            )
            .await
            .unwrap()
            {
                info!("\nHOLEPUNCH SUCCESSFUL\n");
                info!("BOOTSTRAP COMPLETE");
                return Ok(());
            }
        }

        // TODO: we should only attempt to dial the non-common relays
        for relay_address in possible_relays {
            info!(
                "Attempting to connect to relay candidate {} for peer {}",
                relay_address, peer_id
            );
            command_sender
                .send(BootstrapCommand::Dial {
                    multiaddr: relay_address.clone(),
                })
                .await
                .unwrap();

            // we need to know our external IP so we can tell the other node who to holepunch to
            // TODO: a timeout here and also decide if we need block_on
            block_on(async {
                let mut learned_observed_addr = false;
                let mut told_relay_observed_addr = false;

                loop {
                    match event_receiver
                        .recv()
                        .await
                        .context("event sender shouldn't drop")
                        .unwrap()
                    {
                        BootstrapEvent::IdentifySent => {
                            told_relay_observed_addr = true;
                        }
                        BootstrapEvent::IdentifyReceived => {
                            learned_observed_addr = true;
                        }
                        _ => (),
                    }

                    if learned_observed_addr && told_relay_observed_addr {
                        break;
                    }
                }
            });

            // attempt to holepunch to the target peer with the relay we just connected to
            if attempt_holepunch(
                relay_address,
                peer_id,
                command_sender.clone(),
                event_receiver,
            )
            .await
            .unwrap()
            {
                info!("\nHOLEPUNCH SUCCESSFUL\n");
                break;
            }
        }
    }

    info!("BOOTSTRAP COMPLETE");
    Ok(())
}

async fn attempt_holepunch(
    relay_address: Multiaddr,
    target_peer_id: PeerId,
    command_sender: Sender<BootstrapCommand>,
    event_receiver: &mut Receiver<BootstrapEvent>,
) -> Result<bool> {
    // attempt to hole punch to the node we failed to dial earlier
    let multiaddr = relay_address
        .with(Protocol::P2pCircuit)
        .with(Protocol::P2p(target_peer_id));

    command_sender
        .send(BootstrapCommand::Dial { multiaddr })
        .await
        .unwrap();
    info!(peer = ?target_peer_id, "Attempting to hole punch");

    loop {
        match event_receiver
            .recv()
            .await
            .context("event sender shouldn't drop")
            .unwrap()
        {
            // dcutr events.  If its successful break out of the for loop, if its a failure
            // break out of this loop
            BootstrapEvent::DcutrConnectionSuccessful { remote_peer_id } => {
                if remote_peer_id == target_peer_id {
                    return Ok(true);
                }
            }
            BootstrapEvent::DcutrConnectionFailed { remote_peer_id } => {
                if remote_peer_id == target_peer_id {
                    // holepunch unsuccessful
                    return Ok(false);
                }
            }

            // ignore other events
            _ => (),
        }
    }
}

// returns a vector of relays (as peers) that the two lists have in common
// AND a vector of relays (multiaddr) that we aren't connected with (to dial)
fn compare_relay_lists(
    my_relays: HashSet<Peer>,
    their_relays: Vec<Multiaddr>,
) -> (Vec<Peer>, Vec<Multiaddr>) {
    let mut common_relays = Vec::new();
    let mut relays_to_dial = Vec::new();

    let my_relay_map: HashMap<Multiaddr, Peer> = my_relays
        .iter()
        .map(|peer| (peer.multiaddr.clone(), peer.clone()))
        .collect();

    // Iterate through their_relays once
    for multiaddr in their_relays.iter() {
        if let Some(peer) = my_relay_map.get(multiaddr) {
            common_relays.push(peer.clone());
        } else {
            relays_to_dial.push(multiaddr.clone());
        }
    }

    (common_relays, relays_to_dial)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compare_same_relay_lists() {
        let peer_id_a = PeerId::random();

        let my_relays = vec![Peer {
            multiaddr: "/ip4/127.0.0.1/tcp/4001".parse().unwrap(),
            peer_id: peer_id_a,
        }]
        .into_iter()
        .collect();

        let their_relays = vec!["/ip4/127.0.0.1/tcp/4001".parse().unwrap()];

        let (common_relays, relays_to_dial) = compare_relay_lists(my_relays, their_relays);

        assert_eq!(common_relays.len(), 1);
        assert!(relays_to_dial.is_empty());
        assert!(common_relays.contains(&Peer {
            multiaddr: "/ip4/127.0.0.1/tcp/4001".parse().unwrap(),
            peer_id: peer_id_a
        }));
    }

    #[test]
    fn test_empty_my_relay_list() {
        let my_relays = HashSet::new();
        let their_relays = vec!["/ip4/127.0.0.1/tcp/4001".parse().unwrap()];

        let (common_relays, relays_to_dial) = compare_relay_lists(my_relays, their_relays);

        assert!(common_relays.is_empty());
        assert_eq!(relays_to_dial.len(), 1);
        let multiaddr = "/ip4/127.0.0.1/tcp/4001".parse().unwrap();
        assert!(relays_to_dial.contains(&multiaddr));
    }

    #[test]
    fn test_tattered_relay_lists() {
        let peer_id_a = PeerId::random();
        let peer_id_b = PeerId::random();

        let my_relays = vec![
            Peer {
                multiaddr: "/ip4/127.0.0.1/tcp/4001".parse().unwrap(),
                peer_id: peer_id_a,
            },
            Peer {
                multiaddr: "/ip4/142.93.2.49/tcp/4021".parse().unwrap(),
                peer_id: peer_id_b,
            },
        ]
        .into_iter()
        .collect();

        let their_relays = vec![
            "/ip4/127.0.0.1/tcp/4001".parse().unwrap(),
            "/ip4/142.93.53.125/tcp/4021".parse().unwrap(),
        ];

        let (common_relays, relays_to_dial) = compare_relay_lists(my_relays, their_relays);

        assert_eq!(common_relays.len(), 1);
        assert_eq!(relays_to_dial.len(), 1);
        assert!(common_relays.contains(&Peer {
            multiaddr: "/ip4/127.0.0.1/tcp/4001".parse().unwrap(),
            peer_id: peer_id_a,
        }));
        let multiaddr = "/ip4/142.93.53.125/tcp/4021".parse().unwrap();
        assert!(relays_to_dial.contains(&multiaddr));
    }
}
