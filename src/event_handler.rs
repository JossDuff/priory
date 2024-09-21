use crate::{MyBehaviour, MyBehaviourEvent, P2pNode};
use anyhow::Result;
use libp2p::{
    core::{
        multiaddr::{Multiaddr, Protocol},
        ConnectedPoint,
    },
    gossipsub, identify, kad, mdns,
    swarm::{Swarm, SwarmEvent},
    PeerId,
};
use tracing::{info, warn};

// TODO: is it better to take a mut Swarm here or to send Commands??
pub fn handle_swarm_event(
    swarm: &mut Swarm<MyBehaviour>,
    event: SwarmEvent<MyBehaviourEvent>,
) -> Result<()> {
    let _ = match event {
        SwarmEvent::NewListenAddr { address, .. } => {
            let p2p_address = address.with(Protocol::P2p(*swarm.local_peer_id()));
            info!("Listening on {p2p_address}");
        }
        SwarmEvent::ConnectionEstablished {
            peer_id,
            endpoint,
            num_established,
            ..
        } => {
            info!(%peer_id, ?endpoint, %num_established, "Connection Established");
            // TODO: not sure if I need to add both address and send_back_addr.  Seems to
            // work for now
            let multiaddr = match endpoint {
                ConnectedPoint::Dialer { address, .. } => address,
                ConnectedPoint::Listener { send_back_addr, .. } => send_back_addr,
            };
            swarm
                .behaviour_mut()
                .kademlia
                .add_address(&peer_id, multiaddr);
        }
        SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
            warn!("Failed to dial {peer_id:?}: {error}");
        }
        SwarmEvent::IncomingConnectionError { error, .. } => {
            warn!("{:#}", anyhow::Error::from(error));
        }
        SwarmEvent::ConnectionClosed {
            peer_id,
            cause,
            endpoint,
            num_established,
            ..
        } => {
            info!(%peer_id, ?endpoint, %num_established, ?cause, "Connection Closed");
        }
        SwarmEvent::Behaviour(MyBehaviourEvent::Dcutr(event)) => {
            info!("dcutr: {:?}", event);
        }
        SwarmEvent::Behaviour(MyBehaviourEvent::RelayClient(event)) => {
            info!("Relay client: {event:?}");
        } // SwarmEvent::Behaviour(MyBehaviourEvent::RelayClient(
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
            swarm.add_external_address(observed_addr);
        }
        SwarmEvent::Behaviour(MyBehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
            for (peer_id, _multiaddr) in list {
                // println!("mDNS discovered a new peer: {peer_id}");
                // Explicit peers are peers that remain connected and we unconditionally
                // forward messages to, outside of the scoring system.
                swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                // swarm.behaviour_mut().kademlia.add_address(&peer_id, multiaddr);
            }
        }
        SwarmEvent::Behaviour(MyBehaviourEvent::Mdns(mdns::Event::Expired(list))) => {
            for (peer_id, _multiaddr) in list {
                // println!("mDNS discovered peer has expired: {peer_id}");
                swarm
                    .behaviour_mut()
                    .gossipsub
                    .remove_explicit_peer(&peer_id);
                // swarm.behaviour_mut().kademlia.remove_address(&peer_id, &multiaddr);
            }
        }
        SwarmEvent::Behaviour(MyBehaviourEvent::Gossipsub(gossipsub::Event::Message {
            propagation_source: peer_id,
            message_id: id,
            message,
        })) => {
            // TODO: rewire this function to work
            // handle_message(peer_id, message);
            println!(
                "Got message: '{}' with id: {id} from peer: {peer_id}",
                // "{}",
                String::from_utf8_lossy(&message.data),
            );
        }
        SwarmEvent::Behaviour(MyBehaviourEvent::Gossipsub(gossipsub::Event::Subscribed {
            peer_id,
            topic: _,
        })) => {
            info!("{peer_id} subscribed to the topic!");
        }

        SwarmEvent::Behaviour(MyBehaviourEvent::Kademlia(
            kad::Event::OutboundQueryProgressed { result, .. },
        )) => match result {
            kad::QueryResult::GetProviders(Ok(kad::GetProvidersOk::FoundProviders {
                key,
                providers,
                ..
            })) => {
                for peer in providers {
                    println!(
                        "Peer {peer:?} provides key {:?}",
                        std::str::from_utf8(key.as_ref()).unwrap()
                    );
                }
            }
            kad::QueryResult::GetProviders(Err(err)) => {
                eprintln!("Failed to get providers: {err:?}");
            }
            kad::QueryResult::GetRecord(Ok(kad::GetRecordOk::FoundRecord(kad::PeerRecord {
                record: kad::Record { key, value, .. },
                ..
            }))) => {
                println!(
                    "Got record {:?} {:?}",
                    std::str::from_utf8(key.as_ref()).unwrap(),
                    std::str::from_utf8(&value).unwrap(),
                );
            }
            // kad::QueryResult::GetRecord(Ok(_)) => {}
            kad::QueryResult::GetRecord(Err(err)) => {
                eprintln!("Failed to get record: {err:?}");
            }
            kad::QueryResult::PutRecord(Ok(kad::PutRecordOk { key })) => {
                println!(
                    "Successfully put record {:?}",
                    std::str::from_utf8(key.as_ref()).unwrap()
                );
            }
            kad::QueryResult::PutRecord(Err(err)) => {
                eprintln!("Failed to put record: {err:?}");
            }
            kad::QueryResult::StartProviding(Ok(kad::AddProviderOk { key })) => {
                println!(
                    "Successfully put provider record {:?}",
                    std::str::from_utf8(key.as_ref()).unwrap()
                );
            }
            kad::QueryResult::StartProviding(Err(err)) => {
                eprintln!("Failed to put provider record: {err:?}");
            }
            _ => {
                info!("KAD: {:?}", result)
            }
        },

        SwarmEvent::Behaviour(MyBehaviourEvent::Kademlia(kad::Event::RoutingUpdated {
            peer,
            addresses,
            ..
        })) => {
            info!( peer=%peer, addresses=?addresses, "KAD routing table updated");
        }
        _ => (),
    };
    Ok(())
}
