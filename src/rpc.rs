use reqwest::Client;
use tracing::{debug, warn};

use crate::types::*;

pub struct SolanaRpc {
    url: String,
    http: Client,
}

impl SolanaRpc {
    pub fn new(url: &str) -> Self {
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            url: url.to_string(),
            http,
        }
    }

    pub async fn rpc_call<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<T> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });

        let resp = self
            .http
            .post(&self.url)
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("RPC request failed for {method}: {e}"))?;

        let text = resp
            .text()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read RPC response for {method}: {e}"))?;

        let rpc_resp: RpcResponse<T> = serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("Failed to parse RPC response for {method}: {e}"))?;

        if let Some(err) = rpc_resp.error {
            anyhow::bail!("RPC error for {method} (code {}): {}", err.code, err.message);
        }

        rpc_resp
            .result
            .ok_or_else(|| anyhow::anyhow!("No result in RPC response for {method}"))
    }

    /// Fetch all signatures for an address, paginating as needed.
    /// Returns signatures in newest-first order (RPC default).
    pub async fn get_all_signatures(&self, address: &str) -> anyhow::Result<Vec<SignatureInfo>> {
        let mut all_sigs = Vec::new();
        let mut before: Option<String> = None;
        let max_pages = 50; // Safety limit: 50 * 1000 = 50,000 signatures

        for page in 0..max_pages {
            let params = match &before {
                Some(b) => serde_json::json!([address, {"limit": 1000, "before": b}]),
                None => serde_json::json!([address, {"limit": 1000}]),
            };

            let sigs: Vec<SignatureInfo> =
                self.rpc_call("getSignaturesForAddress", params).await?;

            if sigs.is_empty() {
                break;
            }

            let count = sigs.len();
            before = Some(sigs.last().unwrap().signature.clone());
            all_sigs.extend(sigs);

            debug!(page, count, total = all_sigs.len(), "Fetched signature page");
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            if count < 1000 {
                break;
            }
        }

        if all_sigs.len() >= 50_000 {
            warn!(
                "Hit signature pagination limit (50,000). Token may have more transactions."
            );
        }

        Ok(all_sigs)
    }

    /// Fetch a limited number of signatures for an address (newest first).
    pub async fn get_signatures(
        &self,
        address: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<SignatureInfo>> {
        let params = serde_json::json!([address, {"limit": limit}]);
        self.rpc_call("getSignaturesForAddress", params).await
    }

    /// Fetch a single transaction by signature with jsonParsed encoding.
    pub async fn get_transaction(&self, signature: &str) -> anyhow::Result<TransactionResult> {
        let params = serde_json::json!([
            signature,
            {
                "encoding": "jsonParsed",
                "maxSupportedTransactionVersion": 0
            }
        ]);
        self.rpc_call("getTransaction", params).await
    }

    /// Fetch token metadata from pump.fun API.
    pub async fn get_coin_data(&self, mint: &str) -> anyhow::Result<CoinData> {
        let url = format!("https://frontend-api-v3.pump.fun/coins/{}", mint);
        let resp = self.http.get(&url).send().await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Failed to fetch coin data ({status}): {body}");
        }

        Ok(resp.json().await?)
    }
}
