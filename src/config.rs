use anyhow::Result;
use libp2p::{multiaddr::Multiaddr, PeerId};
use serde::Deserialize;
use std::fs;

#[derive(Debug, Deserialize)]
pub struct Config {
    /// If attempting to holepunch, this address will be used as the relay.  
    pub relay_address: Option<Multiaddr>,

    // TODO: make this a vec
    /// peers to connect to on startup
    pub peers: Option<PeerId>,

    /// specify whether or not you will be a relay node for others.  This requires your node is
    /// publically accessible.  Will need to do some sort of test of this
    /// By default, is true
    pub is_relay: Option<bool>,

    // TODO: should have a lower bound and upper bound
    pub peering_degree: u8,

    /// Name that appears when sending a message
    pub name: Option<String>,

    /// Fixed value to generate deterministic peer id
    // TODO: this is for dev work only
    pub secret_key_seed: u8,

    /// The port used to listen on all interfaces
    pub port: u16,
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
        assert_eq!(cfg.is_relay, Some(true));
        assert_eq!(cfg.name, Some("joss!".into()));
        assert_eq!(cfg.secret_key_seed, 1);
        assert_eq!(cfg.relay_address, Some(Multiaddr::from_str("/ip4/142.93.53.125/tcp/4001/p2p/12D3KooWDpJ7As7BWAwRMfu1VU2WCqNjvq387JEYKDBj4kx6nXTN").unwrap()));
        assert_eq!(
            cfg.peers,
            Some(PeerId::from_str("12D3KooWQYhTNQdmr3ArTeUHRYzFg94BKyTkoWBDWez9kSCVe2Xo").unwrap())
        );
        assert_eq!(cfg.peering_degree, 6);
    }
}
