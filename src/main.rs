/**

provide a bootstrap multiaddress (ipv4) to dial
Then on connection add everything to kademlia DHT
Need to know all the types of connection behaviour

**/
use clap::Parser;
use futures::{executor::block_on, future::FutureExt, stream::StreamExt};
use libp2p::core::ConnectedPoint;
use libp2p::kad;
use libp2p::{
    core::{
        multiaddr::{Multiaddr, Protocol},
        Endpoint,
    },
    dcutr, gossipsub, identify, identity,
    kad::store::MemoryStore,
    mdns, noise, ping, relay,
    swarm::NetworkBehaviour,
    swarm::SwarmEvent,
    tcp, yamux, PeerId,
};
use std::collections::hash_map::DefaultHasher;
use std::error::Error;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::str::FromStr;
use std::time::Duration;
use tokio::{io, io::AsyncBufReadExt, select};
use tracing::{debug, error, info, trace, warn};
use tracing_subscriber::EnvFilter;

// custom network behavious that combines gossipsub and mdns
#[derive(NetworkBehaviour)]
struct MyBehaviour {
    gossipsub: gossipsub::Behaviour,
    mdns: mdns::tokio::Behaviour,
    kademlia: kad::Behaviour<MemoryStore>,
    // from dcutr example
    // relay_client: relay::client::Behaviour,
    // ping: ping::Behaviour,
    // // for learning our own addr and telling other nodes their addr
    // identify: identify::Behaviour,
    // // hole punching
    // dcutr: dcutr::Behaviour,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let username = prompt_username();

    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let mut swarm = libp2p::SwarmBuilder::with_new_identity()
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
                .heartbeat_interval(Duration::from_secs(10))
                .validation_mode(gossipsub::ValidationMode::Strict)
                .message_id_fn(message_id_fn)
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

            let kademlia = kad::Behaviour::new(
                keypair.public().to_peer_id(),
                MemoryStore::new(keypair.public().to_peer_id()),
            );
            //
            // let relay_client = relay_behaviour;
            //
            // let ping = ping::Behaviour::new(ping::Config::new());
            //
            // let identify = identify::Behaviour::new(identify::Config::new(
            //     "TODO/0.0.1".to_string(),
            //     keypair.public(),
            // ));
            //
            // let dcutr = dcutr::Behaviour::new(keypair.public().to_peer_id());
            //
            Ok(MyBehaviour {
                gossipsub,
                mdns,
                kademlia,
                // relay_client,
                // ping,
                // identify,
                // dcutr,
            })
        })?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
        .build();

    // create a gossipsub topic
    let topic = gossipsub::IdentTopic::new("test-net");
    // subscribes to our IdentTopic
    swarm.behaviour_mut().gossipsub.subscribe(&topic)?;

    // read full lines from stdin
    let mut stdin = io::BufReader::new(io::stdin()).lines();

    // Listen on all interfaces and whatever port the OS assigns
    swarm.listen_on("/ip4/0.0.0.0/udp/0/quic-v1".parse()?)?;
    swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;

    println!("Enter messages via STDIN and they will be sent to connected peers using Gossipsub");
    println!("To bootstrap Kademlia, type '/bootstrap <multiaddr of external peer>'\n");

    // let it rip
    loop {
        select! {
            Ok(Some(line)) = stdin.next_line() => {
             if let Some(addr) = line.strip_prefix("/bootstrap ") {
                    let addr: libp2p::Multiaddr = addr.parse()?;
                    swarm.dial(addr.clone())?;

                    // TODO: I don't think this should be PeerId::random()
                    // actually, PeerId::random() can be used for randomly walking a DHT so maybe
                    // this will be okay.
                    // Or maybe we should be using the identity protocol to learn the actual
                    // peerid?

                    // swarm.behaviour_mut().kademlia.add_address(&PeerId::random(), addr.clone());
                    // swarm.behaviour_mut().kademlia.bootstrap()?;
                    info!("bootstrapped with address {}", addr);
                } else {
                    let line = format!("{username}: {line}");
                    if let Err(e) = swarm.behaviour_mut().gossipsub.publish(topic.clone(), line.as_bytes()) {
                        warn!("Publish error: {e:?}");
                    }
                }
            }
            event = swarm.select_next_some() => match event {
                SwarmEvent::NewListenAddr { address, .. } => {

                    let p2p_address = address.with(Protocol::P2p(*swarm.local_peer_id()));
                    info!("Listening on {p2p_address}");
                }
                SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                    info!("Connected to {peer_id}");
                }
                SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                    warn!("Failed to dial {peer_id:?}: {error}");
                }
                SwarmEvent::IncomingConnectionError { error, .. } => {
                    warn!("{:#}", anyhow::Error::from(error))
                }
                SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
                    warn!("Connection to {peer_id} closed: {cause:?}");
                    swarm.behaviour_mut().kademlia.remove_peer(&peer_id);
                    info!("Removed {peer_id} from the routing table (if it was in there).");
                }

                SwarmEvent::Behaviour(MyBehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
                    for (peer_id, multiaddr) in list {
                        // println!("mDNS discovered a new peer: {peer_id}");
                        swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                        // swarm.behaviour_mut().kademlia.add_address(&peer_id, multiaddr);
                    }
                },
                SwarmEvent::Behaviour(MyBehaviourEvent::Mdns(mdns::Event::Expired(list))) => {
                    for (peer_id, multiaddr) in list {
                        // println!("mDNS discovered peer has expired: {peer_id}");
                        swarm.behaviour_mut().gossipsub.remove_explicit_peer(&peer_id);
                        // swarm.behaviour_mut().kademlia.remove_address(&peer_id, &multiaddr);
                    }
                }
                SwarmEvent::Behaviour(MyBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                    propagation_source: _peer_id,
                    message_id: _id,
                    message,
                })) => println!(
                        // "Got message: '{}' with id: {id} from peer: {peer_id}",
                        "{}",
                        String::from_utf8_lossy(&message.data),
                    ),
                SwarmEvent::NewListenAddr { ..} => {
                    // println!("Local node is listening on {address}");
                }
                /**
                SwarmEvent::Behaviour(MyBehaviourEvent::Kademlia(kad::Event::OutboundQueryProgressed{
                    result: kad::QueryResult::GetClosestPeers(Ok(kad::GetClosestPeersOk{key, peers})),
                    ..
                })) => {
                    // TODO: are these newly discovered peers?  Should they be added to the
                    // gossipsub??
                    // println!("Closest peers to {:?}: {}", key, peers.join(", "));
                }
                // TODO: in dcutr example the program blocks until it has both learned and told an
                // address.  Is probably just a result of the tutorial
                SwarmEvent::Behaviour(MyBehaviourEvent::Identify(identify::Event::Sent {..})) => {
                    // tracing::info!("Told relay its public address");
                }
                SwarmEvent::Behaviour(MyBehaviourEvent::Identify(identify::Event::Received {
                    info: identify::Info {observed_addr, ..}, ..
                })) => {
                    // tracing::info!(address=%observed_addr, "Relay told us our observed address");
                }
                **/
                _ => {}
            }

        }
    }
}

fn prompt_username() -> String {
    print!("Enter your name: ");
    std::io::stdout().flush().unwrap();

    let mut name = String::new();
    std::io::stdin().read_line(&mut name).unwrap();

    println!();

    name.trim().to_string()
}

// fn generate_ed25519(secret_key_seed: u8) -> identity::Keypair {
//     let mut bytes = [0u8; 32];
//     bytes[0] = secret_key_seed;
//
//     identity::Keypair::ed25519_from_bytes(bytes).expect("only errors on wrong length")
// }
