use anyhow::Result;
use futures::{executor::block_on, stream::StreamExt};
use libp2p::{
    core::{
        multiaddr::{Multiaddr, Protocol},
        ConnectedPoint, PeerId,
    },
    gossipsub::{self, Message},
    identify,
    swarm::{DialError, SwarmEvent},
};
use std::net::Ipv4Addr;
use tokio::{
    sync::mpsc::{self, Sender},
    time::{sleep, Duration},
};
use tracing::{info, warn};

use crate::command::Command;
use crate::config::Config;
use crate::{MyBehaviour, MyBehaviourEvent}

const GOSSIPSUB_TOPIC: &str = "test-net";

// These are the events that we need some information from during bootstrapping.
// When encountered in the main thread, the specified data is copied here and the
// event is also handled by the common handler.
pub enum BootstrapEvent {
    ConnectionEstablished {
        peer_id: PeerId,
        endpoint: ConnectedPoint,
    },
    OutgoingConnectionError {
        error: DialError,
    },
    GossipsubMessage {
        propagation_source: PeerId,
        message: Message,
    },
    IdentifySent,
    IdentifyReceived,
}

impl BootstrapEvent {
    fn try_from_swarm_event(event: SwarmEvent<MyBehaviourEvent>) -> Option<BootstrapEvent> {
        match event {
            SwarmEvent::ConnectionEstablished {
                peer_id, endpoint, ..
            } => Some(BootstrapEvent::ConnectionEstablished { peer_id, endpoint }),
            SwarmEvent::OutgoingConnectionError { error, .. } => {
                Some(BootstrapEvent::OutgoingConnectionError { error })
            }
            SwarmEvent::Behaviour(MyBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                propagation_source,
                message,
                ..
            })) => Some(BootstrapEvent::GossipsubMessage {
                propagation_source,
                message,
            }),
            SwarmEvent::Behaviour(MyBehaviourEvent::Identify(identify::Event::Sent {..})) => Some(BootstrapEvent::IdentifySent),
            SwarmEvent::Behaviour(MyBehaviourEvent::Identify(identify::Event::Received {..})) => Some(BootstrapEvent::IdentifyReceived),
            _ => None,
        }
    }
}

pub async fn bootstrap_swarm(cfg: &Config, sender: Sender<Command>) -> Result<()> {
    // TODO: don't set this in the function cause it can return early because of a '?' and won't
    // ever be set to false cause it won't reach the end
    sender
        .send(Command::UpdateBootstrappingStatus {
            is_bootstrapping: true,
        })
        .await
        .unwrap();

    // create a gossipsub topic
    let topic = gossipsub::IdentTopic::new(GOSSIPSUB_TOPIC);

    // subscribes to our IdentTopic
    sender
        .send(Command::GossipsubSubscribe { topic })
        .await
        .unwrap();

    // Listen on all interfaces and the specified port
    let listen_addr_tcp = Multiaddr::empty()
        .with(Protocol::from(Ipv4Addr::UNSPECIFIED))
        .with(Protocol::Tcp(cfg.port));
    sender
        .send(Command::ListenOn {
            multiaddr: listen_addr_tcp,
        })
        .await
        .unwrap();

    let listen_addr_quic = Multiaddr::empty()
        .with(Protocol::from(Ipv4Addr::UNSPECIFIED))
        .with(Protocol::Udp(cfg.port))
        .with(Protocol::QuicV1);
    sender
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
    for peer_multiaddr in cfg.peers.clone() {
        // dial peer
        // if successful add to DHT
        // if failure wait until we've made contact with the dht and find a peer to holepunch
        sender
            .send(Command::Dial {
                multiaddr: peer_multiaddr,
            })
            .await
            .unwrap();

        loop {
            // TODO: could we get these events from other sources than the above dial?  Should we
            // be checking that the dialed multiaddr is the one we established connection with?
            match swarm.next().await.unwrap() {
                SwarmEvent::ConnectionEstablished {
                    peer_id, endpoint, ..
                } => {
                    // initial dial was successful
                    let peer_multiaddr = match endpoint {
                        ConnectedPoint::Dialer { address, .. } => address,
                        ConnectedPoint::Listener { send_back_addr, .. } => send_back_addr,
                    };
                    swarm
                        .behaviour_mut()
                        .kademlia
                        .add_address(&peer_id, peer_multiaddr);

                    info!(multiaddr=%multiaddr, "initial dial success!");

                    break;
                }
                SwarmEvent::OutgoingConnectionError { error, .. } => {
                    // initial dial was a failure.  Will probably have to hole punch
                    warn!(multiaddr=%multiaddr, error=?error, "initial dial failure");
                    failed_to_dial.push(multiaddr);
                    break;
                }
                unhandled_event => panic!("unhandled event {:?}", unhandled_event),
            }
        }
    }

    // unable to dial any of the nodes
    if failed_to_dial.len() == cfg.peers.len() {
        panic!("Couldn't connect to any adress listed as a peer in the config");
    }

    // TODO: should we handle the [`Event::OutboundQueryProgressed{QueryResult::Bootstrap}`]?? Or
    // at least wait for it to finish.
    sender.send(Command::KademliaBootstrap).await.unwrap();

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
        sender
            .send(Command::GossipsubPublish {
                topic: topic.into(),
                data: query.into(),
            })
            .await
            .unwrap();

        // Wait until we hear a response from a relay claiming they know this peer_id (or timeout)
        let relay_address = block_on(async {
            let mut timeout = futures_timer::Delay::new(std::time::Duration::from_secs(120)).fuse();
            loop {
                futures::select! {
                event = swarm.next() => {
                    if let SwarmEvent::Behaviour(MyBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                        propagation_source: peer_id,
                        message, ..
                })) = event.unwrap() {
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
                            return relay_multiaddr;
                        }
                    }
                }
                }
                _ = timeout => {
                    // TODO: what to do if nobody has this node?
                    panic!("timed out while waiting to hear response for a relay who knows of our target node")
                }
                }
            }
        });

        sender
            .send(Command::Dial {
                multiaddr: relay_address,
            })
            .await
            .unwrap();

        // we need to know our external IP so we can tell the other node who to holepunch to
        block_on(async {
            let mut learned_observed_addr = false;
            let mut told_relay_observed_addr = false;

            loop {
                match swarm.next().await.unwrap() {
                    SwarmEvent::Behaviour(MyBehaviourEvent::Identify(identify::Event::Sent {
                        ..
                    })) => {
                        tracing::info!("Told relay its public address");
                        told_relay_observed_addr = true;
                    }
                    SwarmEvent::Behaviour(MyBehaviourEvent::Identify(
                        identify::Event::Received {
                            info: identify::Info { observed_addr, .. },
                            ..
                        },
                    )) => {
                        tracing::info!(address=%observed_addr, "Relay told us our observed address");
                        learned_observed_addr = true;
                        swarm.add_external_address(observed_addr)
                    }
                    _ => {}
                }

                if learned_observed_addr && told_relay_observed_addr {
                    break;
                }
            }
        });

        // listen mode as well
        let multiaddr = relay_address.with(Protocol::P2pCircuit);
        sender.send(Command::ListenOn { multiaddr }).await.unwrap();

        // attempt to hole punch to the node we failed to dial earlier
        let mutliaddr = relay_address
            .with(Protocol::P2pCircuit)
            .with(Protocol::P2p(peer_id));
        sender.send(Command::Dial { multiaddr }).await.unwrap();
        info!(peer = ?peer_id, "Attempting to hole punch");
    }

    // TODO: connect to some random amount of other nodes ??
    //
    sender
        .send(Command::UpdateBootstrappingStatus {
            is_bootstrapping: false,
        })
        .await
        .unwrap();
    Ok(())
}
