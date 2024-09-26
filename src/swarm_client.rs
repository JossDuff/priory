use anyhow::{Context, Result};
use futures::executor::block_on;
use libp2p::{
    core::{
        multiaddr::{Multiaddr, Protocol},
        PeerId,
    },
    gossipsub::{self, IdentTopic, Message, TopicHash},
    identify,
    swarm::{DialError, SwarmEvent},
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
    find_ipv4, MyBehaviourEvent, P2pNode, Peer, I_HAVE_RELAYS_PREFIX, WANT_RELAY_FOR_PREFIX,
};

#[derive(Clone)]
pub struct SwarmClient {
    gossipsub_topic: IdentTopic,
    command_sender: Sender<SwarmCommand>,
}

impl SwarmClient {
    pub fn new(command_sender: Sender<SwarmCommand>, gossipsub_topic: IdentTopic) -> Self {
        Self {
            command_sender,
            gossipsub_topic,
        }
    }

    pub async fn gossipsub_publish(&self, data: String) -> Result<()> {
        Ok(self
            .command_sender
            .send(SwarmCommand::GossipsubPublish {
                topic: self.gossipsub_topic.clone().into(),
                data: data.into(),
            })
            .await
            .unwrap())
    }

    pub async fn dial(&self, multiaddr: Multiaddr) -> Result<()> {
        Ok(self
            .command_sender
            .send(SwarmCommand::Dial { multiaddr })
            .await
            .unwrap())
    }

    pub async fn my_relays(&self) -> Result<HashSet<Peer>> {
        let (sender, receiver) = oneshot::channel();
        self.command_sender
            .send(SwarmCommand::MyRelays { sender })
            .await
            .unwrap();

        let my_relays = receiver.await.unwrap();

        Ok(my_relays)
    }
}

pub enum SwarmCommand {
    // Gossipsub commands
    GossipsubPublish {
        topic: TopicHash,
        data: Vec<u8>,
    },
    // Swarm commands
    Dial {
        multiaddr: Multiaddr,
    },
    // P2p node commands
    MyRelays {
        sender: oneshot::Sender<HashSet<Peer>>,
    },
}
