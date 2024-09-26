use anyhow::Result;
use futures::stream::StreamExt;
use libp2p::{
    dcutr, gossipsub, identify, identity, kad, mdns,
    multiaddr::Multiaddr,
    noise, relay,
    swarm::{behaviour::toggle::Toggle, NetworkBehaviour},
    tcp, yamux, PeerId, Swarm,
};
use libp2p_kad::store::MemoryStore;
use serde::Deserialize;
use std::collections::{hash_map::DefaultHasher, HashSet};
use std::hash::{Hash, Hasher};
use tokio::{
    io,
    io::AsyncBufReadExt,
    select,
    sync::mpsc::{self},
    time::Duration,
};
use tracing::warn;

use crate::bootstrap::{bootstrap_swarm, handle_bootstrap_command};
use crate::config::Config;
use crate::event_handler::handle_swarm_event;

const IDENTIFY_PROTOCOL_VERSION: &str = "TODO/0.0.1";
pub const GOSSIPSUB_TOPIC: &str = "test-net";

pub const WANT_RELAY_FOR_PREFIX: &str = "WANT RELAY FOR ";
pub const I_HAVE_RELAYS_PREFIX: &str = "I HAVE RELAYS ";

// custom network behavious that combines gossipsub and mdns
#[derive(NetworkBehaviour)]
pub struct MyBehaviour {
    pub gossipsub: gossipsub::Behaviour,
    pub mdns: mdns::tokio::Behaviour,
    pub relay_client: relay::client::Behaviour,
    // some nodes are relay servers for routing messages
    // Some nodes are not relays
    pub toggle_relay: Toggle<relay::Behaviour>,
    // for learning our own addr and telling other nodes their addr
    pub identify: identify::Behaviour,
    // hole punching
    pub dcutr: dcutr::Behaviour,
    // bootstrapping connections
    pub kademlia: kad::Behaviour<MemoryStore>,
    // TODO: can use connection_limits::Behaviour to limit connections by a % of max memory
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq, Hash)]
pub struct Peer {
    pub multiaddr: Multiaddr,
    pub peer_id: PeerId,
}

pub struct P2pNode {
    pub swarm: Swarm<MyBehaviour>,
    pub topic: gossipsub::IdentTopic,
    pub cfg: Config,
    // relays that we're listening on
    pub relays: HashSet<Peer>,
}

impl P2pNode {
    pub fn new(cfg: Config) -> Result<Self> {
        let swarm = build_swarm(&cfg)?;
        let topic = gossipsub::IdentTopic::new(GOSSIPSUB_TOPIC);
        let relays = HashSet::new();

        Ok(Self {
            swarm,
            topic,
            cfg,
            relays,
        })
    }

    pub async fn run(&mut self) -> Result<()> {
        // TODO: how big should the channels be?
        let (bootstrap_command_sender, mut bootstrap_command_receiver) = mpsc::channel(16);
        let (bootstrap_event_sender, mut bootstrap_event_receiver) = mpsc::channel(16);

        // Bootstrap this node into the network
        let cfg = self.cfg.clone();
        let topic = self.topic.clone();
        tokio::spawn(async move {
            bootstrap_swarm(
                cfg,
                bootstrap_command_sender.clone(),
                &mut bootstrap_event_receiver,
                topic,
            )
            .await
            .unwrap();
            // TODO: is this kosher?  We want to drop it when we're done bootstrapping so
            // event_handler doesn't try to send any more events over this channel
            drop(bootstrap_event_receiver);
        });

        // read full lines from stdin
        let mut stdin = io::BufReader::new(io::stdin()).lines();

        // let it rip
        loop {
            select! {
                Some(command) = bootstrap_command_receiver.recv() => handle_bootstrap_command(self, command).unwrap(),
                event = self.swarm.select_next_some() => handle_swarm_event(self, event, &bootstrap_event_sender).await.unwrap(),
                // Writing & line stuff is just for debugging & dev
                Ok(Some(line)) = stdin.next_line() => handle_input_line(self, line).unwrap(),
            };
        }
    }

    pub fn add_relay(&mut self, relay: Peer) {
        self.relays.insert(relay);
    }

    // pub fn send_message()
}

fn generate_ed25519(secret_key_seed: u8) -> identity::Keypair {
    let mut bytes = [0u8; 32];
    bytes[0] = secret_key_seed;

    identity::Keypair::ed25519_from_bytes(bytes).expect("only errors on wrong length")
}

fn handle_input_line(p2p_node: &mut P2pNode, line: String) -> Result<()> {
    if let Err(e) = p2p_node
        .swarm
        .behaviour_mut()
        .gossipsub
        .publish(p2p_node.topic.clone(), line.as_bytes())
    {
        warn!("Publish error: {e:?}");
    }
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
    Ok(())
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

// extract the ipv4 as a &str from a multiaddr
pub fn find_ipv4(multiaddr_str: &str) -> Option<String> {
    // break it up into protocol & addresses
    let multiaddr_parts: Vec<&str> = multiaddr_str.split("/").collect();

    // find location of the string "ip4"
    let ipv4_prefix_index = multiaddr_parts.iter().position(|part| *part == "ip4");

    // the ip follows the prefix "ip4"
    let ipv4_index = match ipv4_prefix_index {
        Some(index) => index + 1,
        None => return None,
    };

    multiaddr_parts.get(ipv4_index).map(|ipv4| ipv4.to_string())
}
