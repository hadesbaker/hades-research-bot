use std::collections::{HashMap, HashSet};
use tracing::{info, warn};

use crate::rpc::SolanaRpc;
use crate::types::*;

const LAMPORTS_PER_SOL: f64 = 1_000_000_000.0;
const ANALYSIS_WINDOW_MINUTES: u64 = 15;
const TOP_TRADES_COUNT: usize = 5;

// ─── Public API ─────────────────────────────────────────────────────────────

pub async fn analyze_token(rpc: &SolanaRpc, mint: &str) -> anyhow::Result<TokenAnalysis> {
    info!(mint, "Starting token analysis");

    // 1. Fetch coin metadata from pump.fun
    let coin = match rpc.get_coin_data(mint).await {
        Ok(c) => {
            info!(name = %c.name, symbol = %c.symbol, "Fetched coin data");
            c
        }
        Err(e) => {
            warn!(%e, "Failed to fetch coin data, using defaults");
            CoinData {
                name: "unknown".into(),
                symbol: "???".into(),
                description: String::new(),
                market_cap: 0.0,
                usd_market_cap: 0.0,
            }
        }
    };

    // 2. Fetch all signatures for the mint (newest first from RPC)
    info!("Fetching transaction signatures for mint...");
    let mut all_sigs = rpc.get_all_signatures(mint).await?;
    info!(count = all_sigs.len(), "Fetched all signatures");

    all_sigs.reverse();
    let all_sigs: Vec<_> = all_sigs.into_iter().filter(|s| s.err.is_none()).collect();
    if all_sigs.is_empty() {
        anyhow::bail!("No successful transactions found for mint {mint}");
    }

    // 3. Determine creation time and filter to analysis window
    let creation_time = all_sigs[0]
        .block_time
        .ok_or_else(|| anyhow::anyhow!("Creation tx has no blockTime"))?;
    let window_end = creation_time + (ANALYSIS_WINDOW_MINUTES * 60) as i64;

    let window_sigs: Vec<_> = all_sigs
        .into_iter()
        .filter(|s| s.block_time.map(|t| t <= window_end).unwrap_or(false))
        .collect();

    info!(
        count = window_sigs.len(),
        creation_time,
        window_end,
        "Filtered to {ANALYSIS_WINDOW_MINUTES}-minute window"
    );

    // 4. Fetch and parse each transaction
    info!("Fetching and parsing {} transactions...", window_sigs.len());
    let mut trades = Vec::new();

    for (i, sig_info) in window_sigs.iter().enumerate() {
        if i > 0 && i % 25 == 0 {
            info!("  parsed {}/{}", i, window_sigs.len());
        }

        match rpc.get_transaction(&sig_info.signature).await {
            Ok(tx) => {
                if let Some(trade) = parse_transaction(&tx, &sig_info.signature, mint, i == 0) {
                    trades.push(trade);
                }
            }
            Err(e) => {
                warn!(sig = %sig_info.signature, %e, "Failed to fetch tx, skipping");
            }
        }

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    info!(count = trades.len(), "Parsed trades");
    if trades.is_empty() {
        anyhow::bail!("No trades parsed for mint {mint}");
    }

    // 5. Identify creator (signer of the first tx)
    let creator_wallet = trades[0].wallet.clone();
    let creation_ts = trades[0].timestamp;
    info!(creator = %creator_wallet, "Identified creator");

    // 6. Build output
    let analysis = build_analysis(mint, &coin, creation_ts, &trades, &creator_wallet);

    Ok(analysis)
}

// ─── Transaction Parsing ────────────────────────────────────────────────────

fn extract_account_keys(tx: &serde_json::Value) -> Vec<AccountKeyInfo> {
    let mut keys = Vec::new();

    if let Some(account_keys) = tx["message"]["accountKeys"].as_array() {
        for key in account_keys {
            if let Some(pubkey) = key["pubkey"].as_str() {
                keys.push(AccountKeyInfo {
                    pubkey: pubkey.to_string(),
                    signer: key["signer"].as_bool().unwrap_or(false),
                });
            } else if let Some(pubkey) = key.as_str() {
                keys.push(AccountKeyInfo {
                    pubkey: pubkey.to_string(),
                    signer: false,
                });
            }
        }
    }

    keys
}

fn find_token_balance(entries: &[TokenBalanceEntry], wallet: &str, mint: &str) -> f64 {
    for entry in entries {
        if entry.owner.as_deref() == Some(wallet) && entry.mint.as_deref() == Some(mint) {
            return entry.ui_token_amount.ui_amount.unwrap_or(0.0);
        }
    }
    0.0
}

fn parse_transaction(
    tx: &TransactionResult,
    signature: &str,
    mint: &str,
    is_first: bool,
) -> Option<Trade> {
    let meta = tx.meta.as_ref()?;
    if meta.err.is_some() {
        return None;
    }

    let timestamp = tx.block_time?;
    let account_keys = extract_account_keys(&tx.transaction);
    if account_keys.is_empty() {
        return None;
    }

    let wallet = account_keys
        .iter()
        .find(|k| k.signer && k.pubkey != mint)
        .map(|k| k.pubkey.clone())?;

    let signer_idx = account_keys.iter().position(|k| k.pubkey == wallet)?;

    let pre_sol = *meta.pre_balances.get(signer_idx)? as f64 / LAMPORTS_PER_SOL;
    let post_sol = *meta.post_balances.get(signer_idx)? as f64 / LAMPORTS_PER_SOL;
    let sol_change = post_sol - pre_sol;

    let pre_tokens = meta.pre_token_balances.as_deref().unwrap_or(&[]);
    let post_tokens = meta.post_token_balances.as_deref().unwrap_or(&[]);

    let pre_token = find_token_balance(pre_tokens, &wallet, mint);
    let post_token = find_token_balance(post_tokens, &wallet, mint);
    let token_change = post_token - pre_token;

    let action = if is_first {
        TradeAction::Create
    } else if token_change > 0.0 {
        TradeAction::Buy
    } else if token_change < 0.0 {
        TradeAction::Sell
    } else {
        TradeAction::Unknown
    };

    let sol_amount = match action {
        TradeAction::Buy | TradeAction::Create => (-sol_change).max(0.0),
        TradeAction::Sell => sol_change.max(0.0),
        TradeAction::Unknown => sol_change.abs(),
    };

    Some(Trade {
        signature: signature.to_string(),
        timestamp,
        wallet,
        action,
        sol_amount,
        token_amount: token_change.abs(),
        token_balance_before: pre_token,
        token_balance_after: post_token,
    })
}

// ─── Build Analysis Output ──────────────────────────────────────────────────

fn build_analysis(
    mint: &str,
    coin: &CoinData,
    creation_ts: i64,
    trades: &[Trade],
    creator: &str,
) -> TokenAnalysis {
    // Creator section: every tx from the creator's wallet in-window (Create/Buy/Sell)
    let creator_trades: Vec<CreatorTrade> = trades
        .iter()
        .filter(|t| t.wallet == creator && !matches!(t.action, TradeAction::Unknown))
        .map(|t| CreatorTrade {
            timestamp: t.timestamp,
            signature: t.signature.clone(),
            action: t.action.clone(),
            sol_amount: round4(t.sol_amount),
            token_amount: round4(t.token_amount),
            ms_from_create: (t.timestamp - creation_ts) * 1000,
        })
        .collect();

    let creator_section = Creator {
        wallet: creator.to_string(),
        trades: creator_trades,
    };

    // Everything below operates on Buy + Sell events only (Create excluded)
    let buys: Vec<&Trade> = trades
        .iter()
        .filter(|t| matches!(t.action, TradeAction::Buy))
        .collect();
    let sells: Vec<&Trade> = trades
        .iter()
        .filter(|t| matches!(t.action, TradeAction::Sell))
        .collect();

    let highlights = build_highlights(&buys, &sells);
    let trade_stats = build_trade_stats(&buys, &sells);

    TokenAnalysis {
        mint: mint.to_string(),
        token_name: coin.name.clone(),
        token_ticker: coin.symbol.clone(),
        pumpfun_url: format!("https://pump.fun/coin/{}", mint),
        created_at_unix: creation_ts,
        analysis_window_minutes: ANALYSIS_WINDOW_MINUTES,
        creator: creator_section,
        highlights,
        trade_stats,
    }
}

fn build_highlights(buys: &[&Trade], sells: &[&Trade]) -> Highlights {
    let total_buy_volume: f64 = buys.iter().map(|t| t.sol_amount).sum();
    let total_sell_volume: f64 = sells.iter().map(|t| t.sol_amount).sum();

    // Count trades per wallet to find the mode wallet
    let mut per_wallet: HashMap<&str, usize> = HashMap::new();
    for t in buys.iter().chain(sells.iter()) {
        *per_wallet.entry(t.wallet.as_str()).or_insert(0) += 1;
    }

    let most_active = per_wallet
        .into_iter()
        .max_by_key(|(_, c)| *c)
        .map(|(addr, count)| MostActiveWallet {
            address: addr.to_string(),
            trade_count: count,
        });

    let top_buys = top_n_trades(buys, TOP_TRADES_COUNT);
    let top_sells = top_n_trades(sells, TOP_TRADES_COUNT);

    let unique_wallets: HashSet<&str> = buys
        .iter()
        .chain(sells.iter())
        .map(|t| t.wallet.as_str())
        .collect();

    Highlights {
        total_trades: buys.len() + sells.len(),
        total_buys: buys.len(),
        total_sells: sells.len(),
        unique_wallets: unique_wallets.len(),
        most_active_wallet: most_active,
        top_buys,
        top_sells,
        total_buy_volume_sol: round4(total_buy_volume),
        total_sell_volume_sol: round4(total_sell_volume),
        net_inflow_sol: round4(total_buy_volume - total_sell_volume),
    }
}

fn top_n_trades(trades: &[&Trade], n: usize) -> Vec<TopTrade> {
    let mut sorted: Vec<&&Trade> = trades.iter().collect();
    sorted.sort_by(|a, b| b.sol_amount.partial_cmp(&a.sol_amount).unwrap_or(std::cmp::Ordering::Equal));
    sorted
        .into_iter()
        .take(n)
        .map(|t| TopTrade {
            wallet: t.wallet.clone(),
            sol_amount: round4(t.sol_amount),
            timestamp: t.timestamp,
        })
        .collect()
}

fn build_trade_stats(buys: &[&Trade], sells: &[&Trade]) -> TradeStats {
    // Buy/sell SOL distributions
    let buy_amounts: Vec<f64> = buys.iter().map(|t| t.sol_amount).collect();
    let sell_amounts: Vec<f64> = sells.iter().map(|t| t.sol_amount).collect();

    let buys_block = stat_block(&buy_amounts);
    let sells_block = stat_block(&sell_amounts);

    // Global inter-trade delays across Buy + Sell events
    let mut all_ts: Vec<i64> = buys
        .iter()
        .chain(sells.iter())
        .map(|t| t.timestamp)
        .collect();
    all_ts.sort();
    let delays_ms_vec: Vec<i64> = all_ts.windows(2).map(|w| (w[1] - w[0]) * 1000).collect();
    let delays_ms = delay_stats(&delays_ms_vec);

    // Sell percentages (fraction of holdings sold per sell event)
    let sell_pcts: Vec<f64> = sells
        .iter()
        .filter(|t| t.token_balance_before > 0.0)
        .map(|t| (t.token_balance_before - t.token_balance_after) / t.token_balance_before)
        .collect();
    let sell_percentages = pct_stats(&sell_pcts);

    let total = buys.len() + sells.len();
    let (buy_probability, sell_probability) = if total > 0 {
        (
            Some(round4(buys.len() as f64 / total as f64)),
            Some(round4(sells.len() as f64 / total as f64)),
        )
    } else {
        (None, None)
    };

    TradeStats {
        buys: buys_block,
        sells: sells_block,
        delays_ms,
        sell_percentages,
        buy_probability,
        sell_probability,
    }
}

// ─── Statistics Helpers ─────────────────────────────────────────────────────

fn stat_block(values: &[f64]) -> StatBlock {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let total: f64 = values.iter().sum();
    StatBlock {
        count: values.len(),
        min: sorted.first().copied().map(round4),
        max: sorted.last().copied().map(round4),
        avg: avg_f64(values).map(round4),
        p25: percentile_f64(&sorted, 25.0).map(round4),
        p75: percentile_f64(&sorted, 75.0).map(round4),
        total_sol: round4(total),
    }
}

fn delay_stats(values: &[i64]) -> DelayStats {
    let mut sorted = values.to_vec();
    sorted.sort();

    DelayStats {
        min: sorted.first().copied(),
        max: sorted.last().copied(),
        avg: if sorted.is_empty() {
            None
        } else {
            Some(sorted.iter().sum::<i64>() / sorted.len() as i64)
        },
        p25: percentile_i64(&sorted, 25.0),
        p75: percentile_i64(&sorted, 75.0),
    }
}

fn pct_stats(values: &[f64]) -> PctStats {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    PctStats {
        min: sorted.first().copied().map(round4),
        max: sorted.last().copied().map(round4),
        avg: avg_f64(values).map(round4),
        p25: percentile_f64(&sorted, 25.0).map(round4),
        p75: percentile_f64(&sorted, 75.0).map(round4),
    }
}

fn percentile_f64(sorted: &[f64], pct: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let idx = ((pct / 100.0) * (sorted.len() - 1) as f64).round() as usize;
    sorted.get(idx).copied()
}

fn percentile_i64(sorted: &[i64], pct: f64) -> Option<i64> {
    if sorted.is_empty() {
        return None;
    }
    let idx = ((pct / 100.0) * (sorted.len() - 1) as f64).round() as usize;
    sorted.get(idx).copied()
}

fn avg_f64(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        None
    } else {
        Some(values.iter().sum::<f64>() / values.len() as f64)
    }
}

fn round4(v: f64) -> f64 {
    (v * 10000.0).round() / 10000.0
}
