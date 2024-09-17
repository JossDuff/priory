use clap::Parser;
use libp2p::{multiaddr::Multiaddr, PeerId};

#[derive(Debug, Parser)]
pub struct Config {
    /// If attempting to holepunch, this address will be used as the relay.  
    #[clap(long)]
    pub relay_address: Option<Multiaddr>,

    // TODO: make this a vec
    /// peers to connect to on startup
    #[clap(long)]
    pub peers: Option<PeerId>,

    // TODO:
    /// specify whether or not you will be a relay node for others.  This requires your node is
    /// publically accessible.
    // #[clap(long)]
    // pub is_relay: bool,

    /// Name that appears when sending a message
    #[clap(long)]
    pub name: Option<String>,

    /// Fixed value to generate deterministic peer id
    // TODO: this is for dev work only
    #[clap(long, default_value_t = fastrand::u8(0..u8::MAX))]
    pub secret_key_seed: u8,

    /// The port used to listen on all interfaces
    #[clap(long, default_value_t = 0)]
    pub port: u16,
}
