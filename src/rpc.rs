use std::collections::HashMap;

use alloy::providers::{DynProvider, Provider, ProviderBuilder, WsConnect};
use anyhow::Result;
use tracing::{info, warn};

use crate::config::Config;
use tempo_alloy::TempoNetwork;

#[derive(Clone)]
pub struct ChainRpc {
    #[allow(dead_code)]
    pub chain_id: u64,
    pub http: Vec<DynProvider<TempoNetwork>>,
    pub ws: Option<DynProvider<TempoNetwork>>,
    #[allow(dead_code)]
    pub urls: Vec<String>,
}

#[derive(Clone)]
pub struct RpcManager {
    chains: HashMap<u64, ChainRpc>,
}

impl RpcManager {
    pub async fn new(config: &Config) -> Result<Self> {
        let mut chains = HashMap::new();

        for (chain_id, urls) in &config.rpc.chains {
            let mut http = Vec::new();
            for url in urls {
                match ProviderBuilder::new_with_network::<TempoNetwork>()
                    .connect(url)
                    .await
                {
                    Ok(provider) => {
                        info!(%chain_id, %url, "connected http provider");
                        http.push(provider.erased());
                    }
                    Err(err) => {
                        warn!(%chain_id, %url, error = %err, "failed to connect http provider");
                    }
                }
            }

            if http.is_empty() {
                anyhow::bail!("no reachable RPC URLs for chain {chain_id}");
            }

            let ws = if config.watcher.use_websocket {
                let ws_url = urls
                    .iter()
                    .find(|url| url.starts_with("ws://") || url.starts_with("wss://"))
                    .cloned()
                    .or_else(|| urls.first().and_then(|url| to_ws_url(url)));

                if let Some(ws_url) = ws_url {
                    match ProviderBuilder::new_with_network::<TempoNetwork>()
                        .connect_ws(WsConnect::new(ws_url.as_str()))
                        .await
                    {
                        Ok(provider) => {
                            info!(%chain_id, url = %ws_url, "connected ws provider");
                            Some(provider.erased())
                        }
                        Err(err) => {
                            warn!(%chain_id, url = %ws_url, error = %err, "failed to connect ws provider");
                            None
                        }
                    }
                } else {
                    None
                }
            } else {
                None
            };

            chains.insert(
                *chain_id,
                ChainRpc {
                    chain_id: *chain_id,
                    http,
                    ws,
                    urls: urls.clone(),
                },
            );
        }

        Ok(Self { chains })
    }

    pub fn chain(&self, chain_id: u64) -> Option<&ChainRpc> {
        self.chains.get(&chain_id)
    }

    pub fn chain_ids(&self) -> Vec<u64> {
        let mut ids: Vec<u64> = self.chains.keys().copied().collect();
        ids.sort_unstable();
        ids
    }
}

fn to_ws_url(url: &str) -> Option<String> {
    if let Some(rest) = url.strip_prefix("https://") {
        return Some(format!("wss://{rest}"));
    }
    if let Some(rest) = url.strip_prefix("http://") {
        return Some(format!("ws://{rest}"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::to_ws_url;

    #[test]
    fn to_ws_url_converts_http() {
        assert_eq!(
            to_ws_url("http://example.com"),
            Some("ws://example.com".to_string())
        );
        assert_eq!(
            to_ws_url("https://example.com"),
            Some("wss://example.com".to_string())
        );
    }

    #[test]
    fn to_ws_url_ignores_ws() {
        assert_eq!(to_ws_url("ws://example.com"), None);
        assert_eq!(to_ws_url("wss://example.com"), None);
    }
}
