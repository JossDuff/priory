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
use anyhow::Result;
use tracing_subscriber::EnvFilter;

mod config;
use config::Config;

mod bootstrap;
mod event_handler;

mod holepuncher;
mod swarm_client;

mod p2p_node;
use p2p_node::P2pNode;

const CONFIG_FILE_PATH: &str = "priory.toml";

#[tokio::main]
async fn main() -> Result<()> {
    let cfg = Config::parse(CONFIG_FILE_PATH)?;

    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let mut p2p_node = P2pNode::new(cfg)?;

    p2p_node.run().await?;

    Ok(())
}
