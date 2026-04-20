use serde::{Deserialize, Serialize};

// ─── RPC Response Types ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct RpcResponse<T> {
    pub result: Option<T>,
    pub error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SignatureInfo {
    pub signature: String,
    #[serde(rename = "blockTime")]
    pub block_time: Option<i64>,
    pub err: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct TransactionResult {
    #[serde(rename = "blockTime")]
    pub block_time: Option<i64>,
    pub meta: Option<TransactionMeta>,
    pub transaction: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct TransactionMeta {
    pub err: Option<serde_json::Value>,
    pub fee: u64,
    #[serde(rename = "preBalances")]
    pub pre_balances: Vec<u64>,
    #[serde(rename = "postBalances")]
    pub post_balances: Vec<u64>,
    #[serde(rename = "preTokenBalances")]
    pub pre_token_balances: Option<Vec<TokenBalanceEntry>>,
    #[serde(rename = "postTokenBalances")]
    pub post_token_balances: Option<Vec<TokenBalanceEntry>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TokenBalanceEntry {
    #[serde(rename = "accountIndex")]
    pub account_index: usize,
    pub mint: Option<String>,
    pub owner: Option<String>,
    #[serde(rename = "uiTokenAmount")]
    pub ui_token_amount: UiTokenAmount,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UiTokenAmount {
    pub amount: String,
    pub decimals: u8,
    #[serde(rename = "uiAmount")]
    pub ui_amount: Option<f64>,
}

// ─── Pump.fun API ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CoinData {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub symbol: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub market_cap: f64,
    #[serde(default)]
    pub usd_market_cap: f64,
}

// ─── Parsed Trade (intermediate) ────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub enum TradeAction {
    Create,
    Buy,
    Sell,
    Unknown,
}

impl std::fmt::Display for TradeAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TradeAction::Create => write!(f, "create"),
            TradeAction::Buy => write!(f, "buy"),
            TradeAction::Sell => write!(f, "sell"),
            TradeAction::Unknown => write!(f, "unknown"),
        }
    }
}

pub struct AccountKeyInfo {
    pub pubkey: String,
    pub signer: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Trade {
    pub signature: String,
    pub timestamp: i64,
    pub wallet: String,
    pub action: TradeAction,
    pub sol_amount: f64,
    pub token_amount: f64,
    pub token_balance_before: f64,
    pub token_balance_after: f64,
}

// ─── Analysis Output ────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct TokenAnalysis {
    pub mint: String,
    pub token_name: String,
    pub token_ticker: String,
    pub pumpfun_url: String,
    pub created_at_unix: i64,
    pub analysis_window_minutes: u64,
    pub creator: Creator,
    pub highlights: Highlights,
    pub trade_stats: TradeStats,
}

#[derive(Debug, Serialize)]
pub struct Creator {
    pub wallet: String,
    pub trades: Vec<CreatorTrade>,
}

#[derive(Debug, Serialize)]
pub struct CreatorTrade {
    pub timestamp: i64,
    pub signature: String,
    pub action: TradeAction,
    pub sol_amount: f64,
    pub token_amount: f64,
    pub ms_from_create: i64,
}

#[derive(Debug, Serialize)]
pub struct Highlights {
    pub total_trades: usize,
    pub total_buys: usize,
    pub total_sells: usize,
    pub unique_wallets: usize,
    pub most_active_wallet: Option<MostActiveWallet>,
    pub top_buys: Vec<TopTrade>,
    pub top_sells: Vec<TopTrade>,
    pub total_buy_volume_sol: f64,
    pub total_sell_volume_sol: f64,
    pub net_inflow_sol: f64,
}

#[derive(Debug, Serialize)]
pub struct MostActiveWallet {
    pub address: String,
    pub trade_count: usize,
}

#[derive(Debug, Serialize)]
pub struct TopTrade {
    pub wallet: String,
    pub sol_amount: f64,
    pub timestamp: i64,
}

#[derive(Debug, Serialize)]
pub struct TradeStats {
    pub buys: StatBlock,
    pub sells: StatBlock,
    pub delays_ms: DelayStats,
    pub sell_percentages: PctStats,
    pub buy_probability: Option<f64>,
    pub sell_probability: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct StatBlock {
    pub count: usize,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub avg: Option<f64>,
    pub p25: Option<f64>,
    pub p75: Option<f64>,
    pub total_sol: f64,
}

#[derive(Debug, Serialize)]
pub struct DelayStats {
    pub min: Option<i64>,
    pub max: Option<i64>,
    pub avg: Option<i64>,
    pub p25: Option<i64>,
    pub p75: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct PctStats {
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub avg: Option<f64>,
    pub p25: Option<f64>,
    pub p75: Option<f64>,
}
