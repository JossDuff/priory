use anyhow::{Context, Result};
use futures::executor::block_on;
use libp2p::{
    core::{
        multiaddr::{Multiaddr, Protocol},
        ConnectedPoint, PeerId,
    },
    gossipsub::{self, Message},
    identify,
    swarm::SwarmEvent,
};
use std::net::Ipv4Addr;
use tokio::{
    sync::mpsc::{Receiver, Sender},
    time::{sleep, Duration},
};
use tracing::info;

use crate::command::Command;
use crate::config::Config;
use crate::{MyBehaviourEvent, AM_RELAY_FOR_PREFIX, WANT_RELAY_FOR_PREFIX};

// These are the events that we need some information from during bootstrapping.
// When encountered in the main thread, the specified data is copied here and the
// event is also handled by the common handler.
pub enum BootstrapEvent {
    ConnectionEstablished {
        peer_id: PeerId,
        endpoint: ConnectedPoint,
    },
    OutgoingConnectionError,
    GossipsubMessage {
        message: Message,
    },
    IdentifySent,
    IdentifyReceived,
}

impl BootstrapEvent {
    pub fn try_from_swarm_event(event: &SwarmEvent<MyBehaviourEvent>) -> Option<BootstrapEvent> {
        match event {
            SwarmEvent::ConnectionEstablished {
                peer_id, endpoint, ..
            } => Some(BootstrapEvent::ConnectionEstablished {
                peer_id: *peer_id,
                endpoint: endpoint.clone(),
            }),
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
            _ => None,
        }
    }
}

pub async fn bootstrap_swarm(
    cfg: Config,
    command_sender: Sender<Command>,
    event_receiver: &mut Receiver<BootstrapEvent>,
    topic: gossipsub::IdentTopic,
) -> Result<()> {
    // subscribes to our IdentTopic
    command_sender
        .send(Command::GossipsubSubscribe {
            topic: topic.clone(),
        })
        .await
        .unwrap();

    // Listen on all interfaces and the specified port
    let listen_addr_tcp = Multiaddr::empty()
        .with(Protocol::from(Ipv4Addr::UNSPECIFIED))
        .with(Protocol::Tcp(cfg.port));
    command_sender
        .send(Command::ListenOn {
            multiaddr: listen_addr_tcp,
        })
        .await
        .unwrap();

    let listen_addr_quic = Multiaddr::empty()
        .with(Protocol::from(Ipv4Addr::UNSPECIFIED))
        .with(Protocol::Udp(cfg.port))
        .with(Protocol::QuicV1);
    command_sender
        .send(Command::ListenOn {
            multiaddr: listen_addr_quic,
        })
        .await
        .unwrap();

    // Wait to listen on all interfaces.
    // Likely listening on all interfaces after a second.
    sleep(Duration::from_secs(1)).await;

    // keep track of the nodes that we'll later have to hole punch into
    let mut failed_to_dial: Vec<Multiaddr> = Vec::new();

    // try to dial all peers in config
    for peer_multiaddr in &cfg.peers.clone() {
        // dial peer
        // if successful add to DHT
        // if failure wait until we've made contact with the dht and find a peer to holepunch
        command_sender
            .send(Command::Dial {
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
                BootstrapEvent::ConnectionEstablished { .. } => {
                    // TODO: have to make sure this event is about the node we just dialed
                    break;
                }

                BootstrapEvent::OutgoingConnectionError { .. } => {
                    // TODO: have to make sure this event is about the node we just dialed
                    failed_to_dial.push(peer_multiaddr.clone());
                    break;
                }
                // ignore other events
                _ => (),
            }
        }
    }

    // unable to dial any of the nodes
    if failed_to_dial.len() == cfg.peers.len() {
        panic!("Couldn't connect to any adress listed as a peer in the config");
    }

    // TODO: should we handle the [`Event::OutboundQueryProgressed{QueryResult::Bootstrap}`]?? Or
    // at least wait for it to finish.
    command_sender
        .send(Command::KademliaBootstrap)
        .await
        .unwrap();

    // TODO: failed_to_dial was initially multiaddrs.  We need to get the peerIds somehow.  Maybe
    // we can get them from the relay that claims to know the peer we want to dial?
    let failed_to_dial: Vec<PeerId> = Vec::new();

    // When DHT is all set and nodes know you're a relay, try to find relays for those nodes you
    // failed to dial earlier
    // FIXME: assuming that the dial failed because its behind a firewall.  Consider other reasons
    // a dial could fail
    // TODO: These should happen concurrently, each on its own tokio thread that handles all the
    // events.  Or maybe it has a channel that receives events and sends to one thread that handles
    // all events?? Hmmm
    for peer_id in failed_to_dial {
        let query = format!("{WANT_RELAY_FOR_PREFIX}{peer_id}");
        command_sender
            .send(Command::GossipsubPublish {
                topic: topic.clone().into(),
                data: query.into(),
            })
            .await
            .unwrap();

        // Wait until we hear a response from a relay claiming they know this peer_id (or timeout)
        let relay_address;
        // TODO: add a timeout (in case nobody is connected to this node) and don't have relay_address be mutable
        loop {
            if let BootstrapEvent::GossipsubMessage { message, .. } = event_receiver
                .recv()
                .await
                .context("event sender shouldn't drop")
                .unwrap()
            {
                let message = String::from_utf8_lossy(&message.data);
                if let Some(str) = message.strip_prefix(AM_RELAY_FOR_PREFIX) {
                    let str: Vec<&str> = str.split(" ").collect();
                    assert!(
                        str.len() == 2,
                        "claims to be relay for an address but returned incorrect info"
                    );
                    let target_peer_id: PeerId = str[0].parse().unwrap();
                    let relay_multiaddr: Multiaddr = str[1].parse().unwrap();

                    // if the relay is talking about the node we care about, return the relay's address
                    if target_peer_id == peer_id {
                        relay_address = relay_multiaddr;
                        break;
                    }
                }
            }
        }

        command_sender
            .send(Command::Dial {
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

        // listen to the dialed relay as well
        let multiaddr = relay_address.clone().with(Protocol::P2pCircuit);
        command_sender
            .send(Command::ListenOn { multiaddr })
            .await
            .unwrap();

        // attempt to hole punch to the node we failed to dial earlier
        let multiaddr = relay_address
            .with(Protocol::P2pCircuit)
            .with(Protocol::P2p(peer_id));
        command_sender
            .send(Command::Dial { multiaddr })
            .await
            .unwrap();
        info!(peer = ?peer_id, "Attempting to hole punch");
    }

    // TODO: connect to some random amount of other nodes ??
    Ok(())
}
