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
[] auto bootstrap when it hits a certain low threshold or receives some error (not enough peers, etc)
**/
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

mod bootstrap;
use bootstrap::bootstrap_swarm;

mod command;
use command::{handle_command, Command};
mod event_handler;
use event_handler::handle_swarm_event;

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

pub struct P2pNode {
    swarm: Swarm<MyBehaviour>,
    cfg: Config,
}

impl P2pNode {
    pub fn new(cfg: Config) -> Result<Self> {
        let swarm = build_swarm(&cfg)?;
        // starts as false, is set to true inside `boostrap_swarm()`
        let is_bootstrapping = false;

        Ok(Self { swarm, cfg })
    }

    pub async fn run(&mut self) -> Result<()> {
        // TODO: how big should the channels be?
        let (command_sender, mut command_receiver) = mpsc::channel(16);
        let (event_sender, mut event_receiver) = mpsc::channel(16);

        // Bootstrap this node into the network
        let cfg = self.cfg.clone();
        tokio::spawn(async move {
            bootstrap_swarm(cfg, command_sender.clone(), &mut event_receiver).await;
        });

        // read full lines from stdin
        let mut stdin = io::BufReader::new(io::stdin()).lines();

        // let it rip
        loop {
            select! {
                Some(command) = command_receiver.recv() => handle_command(self, command),
                event = self.swarm.select_next_some() => handle_swarm_event(self, event, &event_sender).await,
                // Writing & line stuff is just for debugging & dev
                Ok(Some(line)) = stdin.next_line() => handle_input_line(&mut self.swarm, line),
            };
        }
    }

    // pub fn send_message()
}

#[tokio::main]
async fn main() -> Result<()> {
    let cfg = Config::parse(CONFIG_FILE_PATH)?;

    let username = cfg.name;

    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let swarm = build_swarm(&cfg)?;

    Ok(())
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
