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
use std::net::Ipv4Addr;
use tokio::{
    sync::mpsc::{Receiver, Sender},
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
    GossipsubPublish { topic: TopicHash, data: Vec<u8> },
    GossipsubSubscribe { topic: IdentTopic },
    // Swarm commands
    Dial { multiaddr: Multiaddr },
    ListenOn { multiaddr: Multiaddr },
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
    };

    Ok(())
}

pub async fn bootstrap_swarm(
    cfg: Config,
    command_sender: Sender<BootstrapCommand>,
    event_receiver: &mut Receiver<BootstrapEvent>,
    topic: gossipsub::IdentTopic,
) -> Result<()> {
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
                        break;
                    }
                }
            }
        }

        let mut holepunch_successful = false;
        // TODO: we can skip a lot of these steps if we first check if we have any relays in common
        for relay_address in possible_relays {
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

            // attempt to hole punch to the node we failed to dial earlier
            let multiaddr = relay_address
                .with(Protocol::P2pCircuit)
                .with(Protocol::P2p(peer_id));
            command_sender
                .send(BootstrapCommand::Dial { multiaddr })
                .await
                .unwrap();
            info!(peer = ?peer_id, "Attempting to hole punch");

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
                        if remote_peer_id == peer_id {
                            // break out of for loop
                            holepunch_successful = true;
                        }
                    }
                    BootstrapEvent::DcutrConnectionFailed { remote_peer_id } => {
                        if remote_peer_id == peer_id {
                            // break out of this loop
                            break;
                        }
                    }

                    // ignore other events
                    _ => (),
                }
            }

            // break out of the for loop early, we don't need to try every relay
            if holepunch_successful {
                break;
            }
        }
    }

    Ok(())
}
