use anyhow::{Context, Result};
/**

TODO:

[] specify peering degree.  You should be able to connect to only your nodes if you want.  If you have peering degree of 0 your node should still work
[] directly dialing people on dns (/dns/<address> in rust-libp2p/examples/ipfs-kad)
[] kad has bootstrap, but how can we get the bootstrapped connections into gossipsub?
[] automatically discovering & connecting to peers via Kad
[] remove asserts, panics, and unwraps
[] all levels of error handling
[] all levels of tracing logs.  Re-read zero-to-prod logging approach
**/
use futures::FutureExt;
use futures::{executor::block_on, stream::StreamExt};
use libp2p::{
    core::{
        multiaddr::{Multiaddr, Protocol},
        ConnectedPoint, PeerId,
    },
    dcutr, gossipsub,
    gossipsub::{IdentTopic, Message},
    identify, identity, kad, mdns, noise, relay,
    swarm::{behaviour::toggle::Toggle, NetworkBehaviour, SwarmEvent},
    tcp, yamux, Swarm,
};
use libp2p_kad::store::MemoryStore;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::net::Ipv4Addr;
use tokio::{
    io,
    io::AsyncBufReadExt,
    select,
    sync::mpsc::{self, Sender},
    time::{sleep, Duration},
};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

mod config;
use config::Config;

mod command;
use command::{handle_command, Command};
mod event_handler;

const AM_RELAY_FOR_PREFIX: &str = "AM RELAY FOR ";
const WANT_RELAY_FOR_PREFIX: &str = "WANT RELAY FOR ";
const GOSSIPSUB_TOPIC: &str = "test-net";
const IDENTIFY_PROTOCOL_VERSION: &str = "TODO/0.0.1";
const CONFIG_FILE_PATH: &str = "priory.toml";

// custom network behavious that combines gossipsub and mdns
#[derive(NetworkBehaviour)]
pub struct MyBehaviour {
    gossipsub: gossipsub::Behaviour,
    mdns: mdns::tokio::Behaviour,
    relay_client: relay::client::Behaviour,
    // some nodes are relay servers for routing messages
    // Some nodes are not relays
    toggle_relay: Toggle<relay::Behaviour>,
    // for learning our own addr and telling other nodes their addr
    identify: identify::Behaviour,
    // hole punching
    dcutr: dcutr::Behaviour,
    kademlia: kad::Behaviour<MemoryStore>,
    // TODO: can use connection_limits::Behaviour to limit connections by a % of max memory
}

// P2pNode
//  swarm
//  cfg
//  special_handlers
// and there's a client that you get that can send shit on the p2p network

#[tokio::main]
async fn main() -> Result<()> {
    let cfg = Config::parse(CONFIG_FILE_PATH)?;

    let username = cfg.name;

    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let swarm = build_swarm(&cfg)?;

    let (sender, mut receiver) = mpsc::channel(100);

    // Bootstrap this node into the network
    tokio::spawn(async move {
        bootstrap_swarm(&cfg, sender.clone()).await;
    });

    // read full lines from stdin
    let mut stdin = io::BufReader::new(io::stdin()).lines();

    // let it rip
    loop {
        select! {
            Some(command) = receiver.recv() => handle_command(&mut swarm, &command),
            event = swarm.select_next_some() => handle_swarm_event(&mut swarm, &event),
            // Writing & line stuff is just for debugging & dev
            Ok(Some(line)) = stdin.next_line() => handle_input_line(&mut swarm, line),
        }
    }
}

fn generate_ed25519(secret_key_seed: u8) -> identity::Keypair {
    let mut bytes = [0u8; 32];
    bytes[0] = secret_key_seed;

    identity::Keypair::ed25519_from_bytes(bytes).expect("only errors on wrong length")
}

struct RelayConnections {
    pub multiaddr: Multiaddr,
    pub connections: Vec<PeerId>,
}

// TODO: this is just for dev work

fn handle_input_line(swarm: &mut Swarm<MyBehaviour>, line: String) -> Result<()> {
    return Ok(());

    // if let Some(addr) = line.strip_prefix("/bootstrap ") {
    //     let addr: libp2p::Multiaddr = addr.parse()?;
    //     swarm.dial(addr.clone())?;
    //     info!("bootstrapped with address {}", addr);
    // } else if let Some(addr) = line.strip_prefix("/holepunch ") {
    //     let remote_peer_id: PeerId = addr.parse()?;
    //
    //     let relay_addr = match cfg.relay_address.clone() {
    //         Some(a) => a,
    //         None => {
    //             warn!("attempted to hole punch without supplying a relay server address");
    //             continue;
    //         }
    //     };
    //
    //     // Q: will gossipsub auto holepunch for us when a new node joins the network?
    //     swarm
    //         .dial(
    //             relay_addr.clone()
    //                 .with(Protocol::P2pCircuit)
    //                 .with(Protocol::P2p(remote_peer_id)),
    //         )
    //         .unwrap();
    // } else {
    // let line = format!("{username}: {line}");
    // if let Err(e) = swarm.behaviour_mut().gossipsub.publish(topic.clone(), line.as_bytes()) {
    //     warn!("Publish error: {e:?}");
    // }
    // }
    /*
        let mut args = line.split(' ');
        let kademlia = swarm.behaviour_mut().kademlia;

        let _ = match args.next() {
            Some("GET") => {
                let key = {
                    match args.next() {
                        Some(key) => kad::RecordKey::new(&key),
                        None => {
                            eprintln!("Expected key");
                        }
                    }
                };
                kademlia.get_record(key);
            }
            Some("GET_PROVIDERS") => {
                let key = {
                    match args.next() {
                        Some(key) => kad::RecordKey::new(&key),
                        None => {
                            eprintln!("Expected key");
                        }
                    }
                };
                kademlia.get_providers(key);
            }
            Some("PUT") => {
                let key = {
                    match args.next() {
                        Some(key) => kad::RecordKey::new(&key),
                        None => {
                            eprintln!("Expected key");
                        }
                    }
                };
                let value = {
                    match args.next() {
                        Some(value) => value.as_bytes().to_vec(),
                        None => {
                            eprintln!("Expected value");
                        }
                    }
                };
                let record = kad::Record {
                    key,
                    value,
                    publisher: None,
                    expires: None,
                };
                kademlia
                    .put_record(record, kad::Quorum::One)
                    .expect("Failed to store record locally.");
            }
            Some("PUT_PROVIDER") => {
                let key = {
                    match args.next() {
                        Some(key) => kad::RecordKey::new(&key),
                        None => {
                            eprintln!("Expected key");
                        }
                    }
                };

                kademlia
                    .start_providing(key)
                    .expect("Failed to start providing key");
            }
            _ => {
                eprintln!("expected GET, GET_PROVIDERS, PUT or PUT_PROVIDER");
            }
        };

        Ok(())
    */
}

fn handle_message(
    swarm: &mut Swarm<MyBehaviour>,
    propagation_source: PeerId,
    message: Message,
    topic: IdentTopic,
    // TODO: return type
) -> () {
    let message = String::from_utf8_lossy(&message.data);

    // respond to the request if we're a willing relay
    if let Some(target_peer_id) = message.strip_prefix(WANT_RELAY_FOR_PREFIX) {
        // TODO: determine if target_peer_id is listening to us
        let connected_to_target_peer = true;
        // TODO: get my multiaddr
        let my_multiaddress = Multiaddr::empty();
        let is_relay = swarm.behaviour().toggle_relay.is_enabled();

        // broadcast that you're a relay for the target_peer_id
        if is_relay && connected_to_target_peer {
            let response = format!("{AM_RELAY_FOR_PREFIX}{target_peer_id} {my_multiaddress}");

            if let Err(e) = swarm
                .behaviour_mut()
                .gossipsub
                .publish(topic, response.as_bytes())
            {
                warn!("Publish error: {e:?}");
            }
        }
    }
}

fn build_swarm(cfg: &Config) -> Result<Swarm<MyBehaviour>> {
    // deterministically generate a PeerId based on given seed for development ease.
    let local_key: identity::Keypair = generate_ed25519(cfg.secret_key_seed);

    let swarm = libp2p::SwarmBuilder::with_existing_identity(local_key)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_quic()
        .with_dns()?
        .with_relay_client(noise::Config::new, yamux::Config::default)?
        .with_behaviour(|keypair, relay_behaviour| {
            // To content-address messave, we can take the hash of the message and use it as an ID.
            let message_id_fn = |message: &gossipsub::Message| {
                let mut s = DefaultHasher::new();
                message.data.hash(&mut s);
                gossipsub::MessageId::from(s.finish().to_string())
            };

            // Set a custom gossipsub configuration
            let gossipsub_config = gossipsub::ConfigBuilder::default()
                .heartbeat_interval(Duration::from_secs(15)) // This is set to aid debugging by not cluttering the log space
                .validation_mode(gossipsub::ValidationMode::Strict) // This sets the kind of message validation. The default is Strict (enforce message signing)
                .message_id_fn(message_id_fn) // content-address messages. No two messages of the same content will be propagated.
                .mesh_n(cfg.num_gossipsub_connections.mesh_n())
                .mesh_n_low(cfg.num_gossipsub_connections.mesh_n_low())
                .mesh_n_high(cfg.num_gossipsub_connections.mesh_n_high())
                // TODO: figure out what this is about
                // .support_floodsub()
                // .flood_publish(true)
                .build()
                .map_err(|msg| io::Error::new(io::ErrorKind::Other, msg))?;

            let gossipsub = gossipsub::Behaviour::new(
                gossipsub::MessageAuthenticity::Signed(keypair.clone()),
                gossipsub_config,
            )?;

            let mdns = mdns::tokio::Behaviour::new(
                mdns::Config::default(),
                keypair.public().to_peer_id(),
            )?;

            let relay_client = relay_behaviour;

            // if user has indicated they don't want to be a relay, toggle the relay off
            let toggle_relay = if cfg.is_relay {
                Toggle::from(Some(relay::Behaviour::new(
                    keypair.public().to_peer_id(),
                    Default::default(),
                )))
            } else {
                Toggle::from(None)
            };

            let identify = identify::Behaviour::new(identify::Config::new(
                IDENTIFY_PROTOCOL_VERSION.to_string(),
                keypair.public(),
            ));

            let dcutr = dcutr::Behaviour::new(keypair.public().to_peer_id());

            let kademlia = kad::Behaviour::new(
                keypair.public().to_peer_id(),
                MemoryStore::new(keypair.public().to_peer_id()),
            );
            Ok(MyBehaviour {
                gossipsub,
                mdns,
                relay_client,
                toggle_relay,
                identify,
                dcutr,
                kademlia,
            })
        })?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
        .build();

    Ok(swarm)
}

async fn bootstrap_swarm(cfg: &Config, sender: Sender<Command>) -> Result<()> {
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
    Ok(())
}
