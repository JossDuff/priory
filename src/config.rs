use anyhow::Result;
use libp2p::{multiaddr::Multiaddr, PeerId};
use serde::Deserialize;
use std::fs;

#[derive(Debug, Deserialize)]
pub struct Config {
    /// If attempting to holepunch, this address will be used as the relay.  
    // TODO: only for development
    pub relay_address: Option<Multiaddr>,

    // TODO: make this a vec
    /// peers to connect to on startup
    pub peers: Option<PeerId>,

    // TODO: only for development
    #[serde(default = "default_secret_key_seed")]
    pub secret_key_seed: u8,

    /// specify whether or not you will be a relay node for others.
    /// By default, is true
    #[serde(default = "default_is_relay")]
    pub is_relay: bool,

    /// Name that appears when sending a message
    // TODO: only for development
    pub name: String,

    /// The port used to listen on all interfaces
    #[serde(default = "default_port")]
    pub port: u16,

    /// The number of nodes that gossipsub sends full messages to.
    /// The number of gossipsub connections (also called the network degree, D, or peering degree) controls the trade-off
    /// between speed, reliability, resilience and efficiency of the network. A higher peering degree helps messages get
    /// delivered faster, with a better chance of reaching all subscribers and with less chance of any peer disrupting the
    /// network by leaving. However, a high peering degree also causes additional redundant copies of each message to be
    /// sent throughout the network, increasing the bandwidth required to participate in the network.

    /// target number of connections.  Gossipsub will try to form this many connections, but will
    /// accept `gossipsub_target_num_connections_lower_tolerance` less connections or
    /// `gossipsub_target_num_connections_upper_tolerance` more connections
    #[serde(default = "default_gossipsub_target_num_connections")]
    pub gossipsub_target_num_connections: usize,

    /// how many connections under `target_num_connections` is acceptable
    #[serde(default = "default_gossipsub_target_num_connection_lower_tolerance")]
    pub gossipsub_target_num_connections_lower_tolerance: usize,

    /// how many connections above `target_num_connections` is acceptable
    #[serde(default = "default_gossipsub_target_num_connection_upper_tolerance")]
    pub gossipsub_target_num_connections_upper_tolerance: usize,
}

fn default_port() -> u16 {
    4021
}

fn default_is_relay() -> bool {
    true
}

fn default_secret_key_seed() -> u8 {
    fastrand::u8(0..u8::MAX)
}

// try to connect with 6 nodes to send full messages to.
fn default_gossipsub_target_num_connections() -> usize {
    6
}

// default of 1 means 1 less connection than target is acceptable
fn default_gossipsub_target_num_connection_lower_tolerance() -> usize {
    1
}

// default of 6 means 6 more connections than target is acceptable
fn default_gossipsub_target_num_connection_upper_tolerance() -> usize {
    6
}

impl Config {
    pub fn parse(config_file_path: &str) -> Result<Self> {
        let config_content = fs::read_to_string(config_file_path)?;
        let config: Config = toml::from_str(&config_content)?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_parse() {
        let cfg = Config::parse("example_priory.toml").unwrap();

        assert_eq!(cfg.port, 0);
        assert!(cfg.is_relay);
        assert_eq!(cfg.name, "joss!".to_owned());
        assert_eq!(cfg.secret_key_seed, 1);
        assert_eq!(cfg.relay_address, Some(Multiaddr::from_str("/ip4/142.93.53.125/tcp/4001/p2p/12D3KooWDpJ7As7BWAwRMfu1VU2WCqNjvq387JEYKDBj4kx6nXTN").unwrap()));
        assert_eq!(
            cfg.peers,
            Some(PeerId::from_str("12D3KooWQYhTNQdmr3ArTeUHRYzFg94BKyTkoWBDWez9kSCVe2Xo").unwrap())
        );
        assert_eq!(cfg.gossipsub_target_num_connections, 7);
        assert_eq!(cfg.gossipsub_target_num_connections_lower_tolerance, 2);
        assert_eq!(cfg.gossipsub_target_num_connections_upper_tolerance, 5);
    }
}
