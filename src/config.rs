use clap::Parser;
use libp2p::multiaddr::Multiaddr;

#[derive(Debug, Parser)]
pub struct Config {
    /// If attempting to holepunch, this address will be used as the relay.  
    #[clap(long)]
    pub relay_address: Option<Multiaddr>,

    // TODO:
    /// specify whether or not you will be a relay node for others.  This requires your node is
    /// publically accessible.
    // #[clap(long)]
    // is_relay: bool,

    /// Fixed value to generate deterministic peer id
    #[clap(long)]
    pub secret_key_seed: u8,

    /// The port used to listen on all interfaces
    #[clap(long, default_value = "0")]
    pub port: u16,
}
