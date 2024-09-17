use clap::Parser;
/**

TODO:

[] specify peering degree.  You should be able to connect to only your nodes if you want.  If you have peering degree of 0 your node should still work
[] relay in here.  Do you want to be a relay y/n
[] directly dialing people on dns
[] automatically discovering peers via holepunching


**/
use futures::{executor::block_on, stream::StreamExt};
use libp2p::{
    core::multiaddr::{Multiaddr, Protocol},
    dcutr, gossipsub, identify, identity, mdns, noise, relay,
    swarm::NetworkBehaviour,
    swarm::SwarmEvent,
    tcp, yamux, PeerId,
};
use std::collections::hash_map::DefaultHasher;
use std::error::Error;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::net::Ipv4Addr;
use std::time::Duration;
use tokio::{io, io::AsyncBufReadExt, select};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

mod config;
use config::Config;

// custom network behavious that combines gossipsub and mdns
#[derive(NetworkBehaviour)]
struct MyBehaviour {
    gossipsub: gossipsub::Behaviour,
    mdns: mdns::tokio::Behaviour,
    // TODO: I think this doesn't work when all nodes are both relays and relay clients
    relay_client: relay::client::Behaviour,
    // all nodes are relay servers for routing messages
    relay: relay::Behaviour,
    // ping: ping::Behaviour,
    // for learning our own addr and telling other nodes their addr
    identify: identify::Behaviour,
    // hole punching
    dcutr: dcutr::Behaviour,
    // TODO: can use connection_limits::Behaviour to limit connections by a % of max memory
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let cfg = Config::parse();

    let username = prompt_username();

    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    // deterministically generate a PeerId based on given seed for development ease.
    let local_key: identity::Keypair = generate_ed25519(cfg.secret_key_seed);

    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(local_key)
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

            let relay_client = relay_behaviour;

            let relay = relay::Behaviour::new(keypair.public().to_peer_id(), Default::default());

            let identify = identify::Behaviour::new(identify::Config::new(
                "TODO/0.0.1".to_string(),
                keypair.public(),
            ));

            let dcutr = dcutr::Behaviour::new(keypair.public().to_peer_id());

            Ok(MyBehaviour {
                gossipsub,
                mdns,
                relay_client,
                relay,
                identify,
                dcutr,
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

    // Listen on all interfaces and the specified port
    let listen_addr_tcp = Multiaddr::empty()
        .with(Protocol::from(Ipv4Addr::UNSPECIFIED))
        .with(Protocol::Tcp(cfg.port));
    swarm.listen_on(listen_addr_tcp)?;

    let listen_addr_quic = Multiaddr::empty()
        .with(Protocol::from(Ipv4Addr::UNSPECIFIED))
        .with(Protocol::Udp(cfg.port))
        .with(Protocol::QuicV1);
    swarm.listen_on(listen_addr_quic)?;

    // Connect to the relay server. Not for the reservation or relayed connection, but to (a) learn
    // our local public address and (b) enable a freshly started relay to learn its public address.
    if let Some(relay_address) = cfg.relay_address.clone() {
        swarm.dial(relay_address.clone()).unwrap();
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
        swarm
            .listen_on(relay_address.clone().with(Protocol::P2pCircuit))
            .unwrap();
    }

    println!("Enter messages via STDIN and they will be sent to connected peers using Gossipsub");
    println!("To bootstrap, type '/bootstrap <multiaddr of external peer>'");
    println!("To holepunch, type '/holepunch <peer_id of holepunch target>'\n");

    // let it rip
    loop {
        select! {
            Ok(Some(line)) = stdin.next_line() => {
                if let Some(addr) = line.strip_prefix("/bootstrap ") {
                    let addr: libp2p::Multiaddr = addr.parse()?;
                    swarm.dial(addr.clone())?;
                    info!("bootstrapped with address {}", addr);
                } else if let Some(addr) = line.strip_prefix("/holepunch ") {
                    let remote_peer_id: PeerId = addr.parse()?;

                    let relay_addr = match cfg.relay_address.clone() {
                        Some(a) => a,
                        None => {
                            warn!("attempted to hole punch without supplying a relay server address");
                            continue;
                        }
                    };

                    // Q: will gossipsub auto holepunch for us when a new node joins the network?
                    swarm
                        .dial(
                            relay_addr.clone()
                                .with(Protocol::P2pCircuit)
                                .with(Protocol::P2p(remote_peer_id)),
                        )
                        .unwrap();
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
                SwarmEvent::ConnectionEstablished { peer_id, endpoint, num_established, ..} => {
                    info!(%peer_id, ?endpoint, %num_established, "Connection Established")
                }
                SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                    warn!("Failed to dial {peer_id:?}: {error}");
                }
                SwarmEvent::IncomingConnectionError { error, .. } => {
                    warn!("{:#}", anyhow::Error::from(error))
                }
                SwarmEvent::ConnectionClosed { peer_id, cause, endpoint, num_established, ..} => {
                    info!(%peer_id, ?endpoint, %num_established, ?cause, "Connection Closed")
                }
                SwarmEvent::Behaviour(MyBehaviourEvent::Dcutr(event)) => {
                    info!("dcutr: {:?}", event)
                }
                SwarmEvent::Behaviour(MyBehaviourEvent::RelayClient(event) )=> {
                    info!("Relay client: {event:?}")
                }
                // SwarmEvent::Behaviour(MyBehaviourEvent::RelayClient(
                //     relay::client::Event::ReservationReqAccepted { .. },
                // )) => {
                //     info!("Relay accepted our reservation request");
                // }
                // SwarmEvent::Behaviour(MyBehaviourEvent::Identify(identify::Event::Sent {
                //     ..
                // })) => {
                //     // tracing::info!("Told relay its public address");
                // }
                SwarmEvent::Behaviour(MyBehaviourEvent::Identify(identify::Event::Received {
                    info: identify::Info { observed_addr, .. },
                    ..
                })) => {
                    tracing::info!(address=%observed_addr, "Relay told us our observed address");
                    swarm.add_external_address(observed_addr)
                }
                SwarmEvent::Behaviour(MyBehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
                    for (peer_id, _multiaddr) in list {
                        // println!("mDNS discovered a new peer: {peer_id}");
                        // Explicit peers are peers that remain connected and we unconditionally
                        // forward messages to, outside of the scoring system.
                        swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                        // swarm.behaviour_mut().kademlia.add_address(&peer_id, multiaddr);
                    }
                },
                SwarmEvent::Behaviour(MyBehaviourEvent::Mdns(mdns::Event::Expired(list))) => {
                    for (peer_id, _multiaddr) in list {
                        // println!("mDNS discovered peer has expired: {peer_id}");
                        swarm.behaviour_mut().gossipsub.remove_explicit_peer(&peer_id);
                        // swarm.behaviour_mut().kademlia.remove_address(&peer_id, &multiaddr);
                    }
                }
                SwarmEvent::Behaviour(MyBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                    propagation_source: peer_id,
                    message_id: id,
                    message,
                })) => println!(
                        "Got message: '{}' with id: {id} from peer: {peer_id}",
                        // "{}",
                        String::from_utf8_lossy(&message.data),
                    ),
                SwarmEvent::Behaviour(MyBehaviourEvent::Gossipsub(gossipsub::Event::Subscribed {
                    peer_id, topic: _
                })) => info!(
                        "{peer_id} subscribed to the topic!",
                    ),
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

fn generate_ed25519(secret_key_seed: u8) -> identity::Keypair {
    let mut bytes = [0u8; 32];
    bytes[0] = secret_key_seed;

    identity::Keypair::ed25519_from_bytes(bytes).expect("only errors on wrong length")
}
