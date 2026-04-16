use std::collections::{HashMap, HashSet};
use tracing::{debug, info, warn};

use crate::rpc::SolanaRpc;
use crate::types::*;

const LAMPORTS_PER_SOL: f64 = 1_000_000_000.0;
const ANALYSIS_WINDOW_MINUTES: u64 = 15;

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

    // Reverse to chronological order (oldest first)
    all_sigs.reverse();

    // Drop failed transactions
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

    // 5. Identify creator
    let creator_wallet = trades[0].wallet.clone();
    info!(creator = %creator_wallet, "Identified creator");

    // 6. Find primary wallet (creator's funding source)
    info!("Tracing creator's funding source...");
    let (primary_wallet, creator_fund_sol) = find_primary_wallet(rpc, &creator_wallet).await;
    if let Some(ref pw) = primary_wallet {
        info!(
            primary = %pw,
            fund = format!("{:.4}", creator_fund_sol.unwrap_or(0.0)),
            "Identified primary wallet"
        );
    } else {
        warn!("Could not identify primary wallet");
    }

    // 7. Classify trading wallets (bot vs external) by wallet freshness
    let unique_wallets: Vec<String> = trades
        .iter()
        .map(|t| t.wallet.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .filter(|w| *w != creator_wallet)
        .collect();

    info!(
        count = unique_wallets.len(),
        "Classifying trading wallets..."
    );

    let bot_wallet_funds = classify_wallets_by_freshness(
        rpc,
        &unique_wallets,
        creation_time,
    )
    .await;

    let bot_wallets: HashSet<String> = bot_wallet_funds.keys().cloned().collect();
    let external_wallets: Vec<String> = unique_wallets
        .iter()
        .filter(|w| !bot_wallets.contains(*w))
        .cloned()
        .collect();

    info!(
        bot = bot_wallets.len(),
        external = external_wallets.len(),
        "Wallet classification complete"
    );

    // 8. Classify bot wallets into initial vs staggered
    let (initial_wallets, staggered_wallets) =
        classify_bot_phases(&trades, &creator_wallet, &bot_wallets, creation_time);

    info!(
        initial = initial_wallets.len(),
        staggered = staggered_wallets.len(),
        "Phase classification complete"
    );

    // 9. Build the full analysis
    let analysis = build_analysis(
        mint,
        &coin,
        creation_time,
        &trades,
        &creator_wallet,
        &primary_wallet,
        creator_fund_sol,
        &initial_wallets,
        &staggered_wallets,
        &external_wallets,
        &bot_wallet_funds,
    );

    Ok(analysis)
}

// ─── Transaction Parsing ────────────────────────────────────────────────────

fn extract_account_keys(tx: &serde_json::Value) -> Vec<AccountKeyInfo> {
    let mut keys = Vec::new();

    if let Some(account_keys) = tx["message"]["accountKeys"].as_array() {
        for key in account_keys {
            if let Some(pubkey) = key["pubkey"].as_str() {
                // Parsed format: { pubkey, signer, writable, source }
                keys.push(AccountKeyInfo {
                    pubkey: pubkey.to_string(),
                    signer: key["signer"].as_bool().unwrap_or(false),
                });
            } else if let Some(pubkey) = key.as_str() {
                // Legacy string format (fallback)
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

    // Skip failed txs
    if meta.err.is_some() {
        return None;
    }

    let timestamp = tx.block_time?;
    let account_keys = extract_account_keys(&tx.transaction);
    if account_keys.is_empty() {
        return None;
    }

    // Find the trader: first signer that isn't the mint itself
    let wallet = account_keys
        .iter()
        .find(|k| k.signer && k.pubkey != mint)
        .map(|k| k.pubkey.clone())?;

    // Signer's index in account_keys (for balance lookup)
    let signer_idx = account_keys.iter().position(|k| k.pubkey == wallet)?;

    // SOL balance change
    let pre_sol = *meta.pre_balances.get(signer_idx)? as f64 / LAMPORTS_PER_SOL;
    let post_sol = *meta.post_balances.get(signer_idx)? as f64 / LAMPORTS_PER_SOL;
    let sol_change = post_sol - pre_sol;

    // Token balance change
    let pre_tokens = meta
        .pre_token_balances
        .as_deref()
        .unwrap_or(&[]);
    let post_tokens = meta
        .post_token_balances
        .as_deref()
        .unwrap_or(&[]);

    let pre_token = find_token_balance(pre_tokens, &wallet, mint);
    let post_token = find_token_balance(post_tokens, &wallet, mint);
    let token_change = post_token - pre_token;

    // Classify action
    let action = if is_first {
        TradeAction::Create
    } else if token_change > 0.0 {
        TradeAction::Buy
    } else if token_change < 0.0 {
        TradeAction::Sell
    } else {
        TradeAction::Unknown
    };

    // SOL amount (positive = cost for buys, proceeds for sells)
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

// ─── Primary Wallet Detection ───────────────────────────────────────────────

/// Look at the creator wallet's transaction history to find who funded it.
/// Uses raw SOL balance changes so it works regardless of how SOL was transferred
/// (system transfer, Jito bundle, program-level transfer, etc.).
/// Returns (primary_wallet_pubkey, funding_amount_sol).
async fn find_primary_wallet(
    rpc: &SolanaRpc,
    creator: &str,
) -> (Option<String>, Option<f64>) {
    let sigs = match rpc.get_signatures(creator, 50).await {
        Ok(s) => s,
        Err(e) => {
            warn!(%e, "Failed to get creator signatures");
            return (None, None);
        }
    };

    // Iterate from oldest to newest, looking for a tx where creator received SOL
    for sig_info in sigs.iter().rev() {
        if sig_info.err.is_some() {
            continue;
        }

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let tx = match rpc.get_transaction(&sig_info.signature).await {
            Ok(t) => t,
            Err(_) => continue,
        };

        if let Some((sender, amount)) = find_sol_inflow(&tx, creator) {
            if sender != creator && amount > 0.001 {
                return (Some(sender), Some(amount));
            }
        }
    }

    (None, None)
}

/// Detect a SOL inflow to a wallet using raw balance changes.
/// Works for system transfers, Jito bundles, program-level transfers, etc.
/// Returns (likely_sender, amount_sol) where likely_sender is the account
/// whose SOL balance decreased the most in the same transaction.
fn find_sol_inflow(tx: &TransactionResult, to: &str) -> Option<(String, f64)> {
    let meta = tx.meta.as_ref()?;
    let account_keys = extract_account_keys(&tx.transaction);

    // Find the target wallet's account index
    let to_idx = account_keys.iter().position(|k| k.pubkey == to)?;

    let to_pre = *meta.pre_balances.get(to_idx)? as f64;
    let to_post = *meta.post_balances.get(to_idx)? as f64;
    let inflow = to_post - to_pre;

    // Must have received a meaningful amount of SOL
    if inflow < 1_000_000.0 {
        // < 0.001 SOL
        return None;
    }

    // Find the account whose balance decreased the most (likely the sender)
    let mut best_sender: Option<(String, f64)> = None;
    for (i, key) in account_keys.iter().enumerate() {
        if i == to_idx {
            continue;
        }
        let pre = *meta.pre_balances.get(i).unwrap_or(&0) as f64;
        let post = *meta.post_balances.get(i).unwrap_or(&0) as f64;
        let decrease = pre - post;

        if decrease > 1_000_000.0 {
            if best_sender.is_none() || decrease > best_sender.as_ref().unwrap().1 {
                best_sender = Some((key.pubkey.clone(), decrease));
            }
        }
    }

    let amount_sol = inflow / LAMPORTS_PER_SOL;

    match best_sender {
        Some((sender, _)) => Some((sender, amount_sol)),
        None => Some(("unknown".to_string(), amount_sol)),
    }
}

/// Classify trading wallets as bot or external based on wallet freshness.
///
/// Bot wallets are disposable — created specifically for this token launch:
///   - Few total transactions (< 50 lifetime)
///   - Created shortly before the token (oldest tx within 30 min of creation)
///
/// External wallets are established — they existed before this token:
///   - Many transactions (50+), OR
///   - Oldest transaction is well before the token launch
///
/// For identified bot wallets, funding amount is determined from the wallet's
/// earliest transaction using raw SOL balance changes (works for all transfer types).
async fn classify_wallets_by_freshness(
    rpc: &SolanaRpc,
    trading_wallets: &[String],
    creation_time: i64,
) -> HashMap<String, Option<f64>> {
    let mut bot_wallets: HashMap<String, Option<f64>> = HashMap::new();

    // Bot wallets must have been created within this window before token launch
    let earliest_creation = creation_time - 1800; // 30 minutes before

    for (i, wallet) in trading_wallets.iter().enumerate() {
        if i > 0 && i % 50 == 0 {
            info!(
                "  checked {}/{} wallets ({} bot so far)",
                i,
                trading_wallets.len(),
                bot_wallets.len()
            );
        }

        // 1. Get wallet's transaction history
        let sigs = match rpc.get_signatures(wallet, 50).await {
            Ok(s) => s,
            Err(_) => continue,
        };

        // 2. Established wallets have 50+ txs → external
        if sigs.len() >= 50 {
            continue;
        }

        // 3. Check the wallet's oldest transaction time
        let oldest_sig = match sigs.last() {
            Some(s) => s,
            None => continue,
        };

        let oldest_time = match oldest_sig.block_time {
            Some(t) => t,
            None => continue,
        };

        // 4. If the wallet existed before the launch window → external
        if oldest_time < earliest_creation {
            continue;
        }

        // 5. This is a fresh wallet created near launch time → bot wallet
        //    Determine funding amount from the oldest transaction's balance change
        let fund_sol = determine_wallet_funding(rpc, wallet, oldest_sig).await;

        debug!(
            wallet = %wallet,
            txs = sigs.len(),
            fund = format!("{:.4}", fund_sol.unwrap_or(0.0)),
            "Bot wallet (fresh, created near launch)"
        );
        bot_wallets.insert(wallet.clone(), fund_sol);

        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    info!(bot_wallets = bot_wallets.len(), "Wallet classification complete");
    bot_wallets
}

/// Determine how much SOL a bot wallet was funded with by looking at its
/// oldest transaction's balance change.
async fn determine_wallet_funding(
    rpc: &SolanaRpc,
    wallet: &str,
    oldest_sig: &SignatureInfo,
) -> Option<f64> {
    if oldest_sig.err.is_some() {
        return None;
    }

    let tx = rpc.get_transaction(&oldest_sig.signature).await.ok()?;
    let meta = tx.meta.as_ref()?;
    let account_keys = extract_account_keys(&tx.transaction);

    let wallet_idx = account_keys.iter().position(|k| k.pubkey == wallet)?;
    let pre = *meta.pre_balances.get(wallet_idx)? as f64 / LAMPORTS_PER_SOL;
    let post = *meta.post_balances.get(wallet_idx)? as f64 / LAMPORTS_PER_SOL;

    let increase = post - pre;
    if increase > 0.001 {
        Some(increase)
    } else {
        // The oldest tx might be the buy itself (bundled funding+buy).
        // In that case the postBalance is the remaining SOL after buying.
        // Use postBalance as a lower bound for funding.
        if post > 0.001 {
            Some(post)
        } else {
            None
        }
    }
}

// ─── Wallet Phase Classification ────────────────────────────────────────────

/// Split bot wallets into initial buyers vs staggered buyers.
///
/// Heuristic: initial buyers make exactly ONE buy, very soon after creation,
/// with minimal delay between consecutive initial buyer buys.
/// Staggered buyers make multiple trades spread over time.
fn classify_bot_phases(
    trades: &[Trade],
    creator: &str,
    bot_wallets: &HashSet<String>,
    creation_time: i64,
) -> (Vec<String>, Vec<String>) {
    if bot_wallets.is_empty() {
        return (Vec::new(), Vec::new());
    }

    // Count buys per bot wallet and find their first buy timestamp
    let mut wallet_buy_count: HashMap<String, usize> = HashMap::new();
    let mut wallet_first_buy: HashMap<String, i64> = HashMap::new();
    let mut wallet_trade_count: HashMap<String, usize> = HashMap::new();

    for trade in trades {
        if !bot_wallets.contains(&trade.wallet) || trade.wallet == creator {
            continue;
        }

        *wallet_trade_count.entry(trade.wallet.clone()).or_insert(0) += 1;

        if matches!(trade.action, TradeAction::Buy) {
            *wallet_buy_count.entry(trade.wallet.clone()).or_insert(0) += 1;
            wallet_first_buy
                .entry(trade.wallet.clone())
                .or_insert(trade.timestamp);
        }
    }

    // Collect bot wallet buy events, sorted by time
    let mut bot_buys: Vec<(i64, String)> = Vec::new();
    for trade in trades {
        if bot_wallets.contains(&trade.wallet)
            && trade.wallet != creator
            && matches!(trade.action, TradeAction::Buy)
        {
            // Only the FIRST buy per wallet matters for initial buyer detection
            if bot_buys.iter().any(|(_, w)| w == &trade.wallet) {
                continue;
            }
            bot_buys.push((trade.timestamp, trade.wallet.clone()));
        }
    }
    bot_buys.sort_by_key(|(t, _)| *t);

    // Detect the "burst" phase: consecutive first-buys with < 3s gap
    // The burst typically happens right after the creator buy
    let burst_gap_threshold_s = 3;
    let mut initial_wallets: Vec<String> = Vec::new();
    let mut burst_ended = false;

    for i in 0..bot_buys.len() {
        if burst_ended {
            break;
        }

        let (ts, ref wallet) = bot_buys[i];

        // Only single-buy wallets can be initial buyers
        let buy_count = wallet_buy_count.get(wallet).copied().unwrap_or(0);
        let _trade_count = wallet_trade_count.get(wallet).copied().unwrap_or(0);

        // Initial buyers: exactly 1 buy, no sells (during the trading phase)
        // They might have 1 sell at the end (exit), so check buy_count == 1
        if buy_count != 1 {
            burst_ended = true;
            continue;
        }

        if i == 0 {
            // First bot buy — check gap from creation time
            let gap = ts - creation_time;
            if gap <= 30 {
                // Within 30 seconds of creation — likely initial buyer
                initial_wallets.push(wallet.clone());
            } else {
                burst_ended = true;
            }
        } else {
            let prev_ts = bot_buys[i - 1].0;
            let gap = ts - prev_ts;
            if gap <= burst_gap_threshold_s {
                initial_wallets.push(wallet.clone());
            } else {
                burst_ended = true;
            }
        }
    }

    // Everything else is a staggered buyer
    let initial_set: HashSet<String> = initial_wallets.iter().cloned().collect();
    let staggered_wallets: Vec<String> = bot_wallets
        .iter()
        .filter(|w| !initial_set.contains(*w))
        .cloned()
        .collect();

    (initial_wallets, staggered_wallets)
}

// ─── Build Analysis Output ──────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn build_analysis(
    mint: &str,
    coin: &CoinData,
    creation_time: i64,
    trades: &[Trade],
    creator: &str,
    primary_wallet: &Option<String>,
    creator_fund_sol: Option<f64>,
    initial_wallets: &[String],
    staggered_wallets: &[String],
    external_wallets: &[String],
    bot_wallet_funds: &HashMap<String, Option<f64>>,
) -> TokenAnalysis {
    let initial_set: HashSet<&str> = initial_wallets.iter().map(|s| s.as_str()).collect();
    let staggered_set: HashSet<&str> = staggered_wallets.iter().map(|s| s.as_str()).collect();
    let external_set: HashSet<&str> = external_wallets.iter().map(|s| s.as_str()).collect();

    // ── Creator info ────────────────────────────────────────────────────────
    let creator_buy = trades
        .iter()
        .find(|t| t.wallet == creator && matches!(t.action, TradeAction::Buy));

    let create_ts = trades[0].timestamp;
    let creator_buy_delay = creator_buy.map(|b| (b.timestamp - create_ts) * 1000);

    let creator_info = CreatorInfo {
        wallet: creator.to_string(),
        fund_sol: creator_fund_sol,
        buy_amount_sol: creator_buy.map(|b| b.sol_amount),
        buy_delay_after_create_ms: creator_buy_delay,
    };

    // ── Initial buyers ──────────────────────────────────────────────────────
    let mut initial_buyer_infos: Vec<WalletBuyInfo> = Vec::new();
    for wallet in initial_wallets {
        if let Some(trade) = trades
            .iter()
            .find(|t| t.wallet == *wallet && matches!(t.action, TradeAction::Buy))
        {
            initial_buyer_infos.push(WalletBuyInfo {
                pubkey: wallet.clone(),
                fund_sol: bot_wallet_funds.get(wallet).copied().flatten(),
                buy_amount_sol: trade.sol_amount,
                timestamp: trade.timestamp,
            });
        }
    }

    let initial_buy_amounts: Vec<f64> = initial_buyer_infos.iter().map(|w| w.buy_amount_sol).collect();
    let initial_total: f64 = initial_buy_amounts.iter().sum();

    let initial_info = InitialBuyerInfo {
        count: initial_wallets.len(),
        wallets: initial_buyer_infos,
        min_buy_sol: min_f64(&initial_buy_amounts),
        max_buy_sol: max_f64(&initial_buy_amounts),
        total_sol: initial_total,
    };

    // ── Staggered buyers ────────────────────────────────────────────────────
    let mut stag_buy_amounts: Vec<f64> = Vec::new();
    let mut stag_sell_pcts: Vec<f64> = Vec::new();
    let mut stag_total_buy_sol = 0.0;
    let mut stag_total_sell_sol = 0.0;
    let mut stag_buy_count = 0usize;
    let mut stag_sell_count = 0usize;

    // All staggered trades sorted by time for delay calculation
    let mut stag_trade_timestamps: Vec<i64> = Vec::new();

    for trade in trades {
        if !staggered_set.contains(trade.wallet.as_str()) {
            continue;
        }
        match trade.action {
            TradeAction::Buy => {
                stag_buy_count += 1;
                stag_buy_amounts.push(trade.sol_amount);
                stag_total_buy_sol += trade.sol_amount;
                stag_trade_timestamps.push(trade.timestamp);
            }
            TradeAction::Sell => {
                stag_sell_count += 1;
                stag_total_sell_sol += trade.sol_amount;
                stag_trade_timestamps.push(trade.timestamp);

                // Compute sell percentage
                if trade.token_balance_before > 0.0 {
                    let pct =
                        (trade.token_balance_before - trade.token_balance_after) / trade.token_balance_before;
                    stag_sell_pcts.push(pct);
                }
            }
            _ => {}
        }
    }

    stag_trade_timestamps.sort();

    // Compute delays between consecutive staggered trades
    let delays_ms: Vec<i64> = stag_trade_timestamps
        .windows(2)
        .map(|w| (w[1] - w[0]) * 1000)
        .filter(|d| *d > 0)
        .collect();

    let stag_fund_amounts: Vec<f64> = staggered_wallets
        .iter()
        .filter_map(|w| bot_wallet_funds.get(w).copied().flatten())
        .collect();

    let buy_probability = if stag_buy_count + stag_sell_count > 0 {
        Some(stag_buy_count as f64 / (stag_buy_count + stag_sell_count) as f64)
    } else {
        None
    };

    let staggered_info = StaggeredBuyerInfo {
        count: staggered_wallets.len(),
        total_buys: stag_buy_count,
        total_sells: stag_sell_count,
        min_buy_sol: min_f64(&stag_buy_amounts),
        max_buy_sol: max_f64(&stag_buy_amounts),
        total_buy_sol: stag_total_buy_sol,
        total_sell_sol: stag_total_sell_sol,
        fund_per_wallet_sol: avg_f64(&stag_fund_amounts),
        delays_ms: delays_ms.clone(),
        min_delay_ms: delays_ms.iter().copied().min(),
        max_delay_ms: delays_ms.iter().copied().max(),
        avg_delay_ms: if delays_ms.is_empty() {
            None
        } else {
            Some(delays_ms.iter().sum::<i64>() / delays_ms.len() as i64)
        },
        buy_probability,
        sell_percentages: stag_sell_pcts.clone(),
        sell_pct_min: min_f64(&stag_sell_pcts),
        sell_pct_max: max_f64(&stag_sell_pcts),
    };

    // ── External activity ───────────────────────────────────────────────────
    let mut ext_buy_count = 0usize;
    let mut ext_sell_count = 0usize;
    let mut ext_buy_sol = 0.0;
    let mut ext_sell_sol = 0.0;

    for trade in trades {
        if !external_set.contains(trade.wallet.as_str()) {
            continue;
        }
        match trade.action {
            TradeAction::Buy => {
                ext_buy_count += 1;
                ext_buy_sol += trade.sol_amount;
            }
            TradeAction::Sell => {
                ext_sell_count += 1;
                ext_sell_sol += trade.sol_amount;
            }
            _ => {}
        }
    }

    let external_info = ExternalInfo {
        unique_wallets: external_wallets.len(),
        total_buys: ext_buy_count,
        total_sells: ext_sell_count,
        total_buy_sol: ext_buy_sol,
        total_sell_sol: ext_sell_sol,
    };

    // ── Trade timeline ──────────────────────────────────────────────────────
    let timeline: Vec<TimelineEntry> = trades
        .iter()
        .filter(|t| !matches!(t.action, TradeAction::Unknown))
        .map(|t| {
            let role = if t.wallet == creator {
                "creator"
            } else if initial_set.contains(t.wallet.as_str()) {
                "initial_buyer"
            } else if staggered_set.contains(t.wallet.as_str()) {
                "staggered_buyer"
            } else {
                "external"
            };
            TimelineEntry {
                timestamp: t.timestamp,
                signature: t.signature.clone(),
                wallet: t.wallet.clone(),
                wallet_role: role.to_string(),
                action: t.action.clone(),
                sol_amount: round4(t.sol_amount),
                token_amount: round4(t.token_amount),
            }
        })
        .collect();

    // ── Transaction counts ──────────────────────────────────────────────────
    let bot_tx_count = trades
        .iter()
        .filter(|t| {
            t.wallet == creator
                || initial_set.contains(t.wallet.as_str())
                || staggered_set.contains(t.wallet.as_str())
        })
        .count();
    let ext_tx_count = trades
        .iter()
        .filter(|t| external_set.contains(t.wallet.as_str()))
        .count();

    // ── Profitability ──────────────────────────────────────────────────────
    let profitability = compute_profitability(trades, creator, &initial_set, &staggered_set, &external_info);

    // ── Assumed config ──────────────────────────────────────────────────────
    let assumed_config = build_assumed_config(
        &creator_info,
        &initial_info,
        &staggered_info,
        &stag_fund_amounts,
    );

    TokenAnalysis {
        mint: mint.to_string(),
        token_name: coin.name.clone(),
        token_ticker: coin.symbol.clone(),
        pumpfun_url: format!("https://pump.fun/coin/{}", mint),
        created_at_unix: creation_time,
        analysis_window_minutes: ANALYSIS_WINDOW_MINUTES,
        total_transactions: trades.len(),
        total_bot_transactions: bot_tx_count,
        total_external_transactions: ext_tx_count,
        primary_wallet: primary_wallet.clone(),
        creator: creator_info,
        initial_buyers: initial_info,
        staggered_buyers: staggered_info,
        external_activity: external_info,
        profitability,
        trade_timeline: timeline,
        assumed_config,
    }
}

fn compute_profitability(
    trades: &[Trade],
    creator: &str,
    initial_set: &HashSet<&str>,
    staggered_set: &HashSet<&str>,
    external: &ExternalInfo,
) -> Profitability {
    let mut creator_spent = 0.0;
    let mut creator_received = 0.0;
    let mut bot_spent = 0.0;
    let mut bot_received = 0.0;

    for trade in trades {
        let is_bot = trade.wallet == creator
            || initial_set.contains(trade.wallet.as_str())
            || staggered_set.contains(trade.wallet.as_str());

        match trade.action {
            TradeAction::Create | TradeAction::Buy => {
                if trade.wallet == creator {
                    creator_spent += trade.sol_amount;
                }
                if is_bot {
                    bot_spent += trade.sol_amount;
                }
            }
            TradeAction::Sell => {
                if trade.wallet == creator {
                    creator_received += trade.sol_amount;
                }
                if is_bot {
                    bot_received += trade.sol_amount;
                }
            }
            _ => {}
        }
    }

    let creator_net = creator_received - creator_spent;
    let bot_net = bot_received - bot_spent;
    let external_net_buy = external.total_buy_sol - external.total_sell_sol;
    let estimated_overhead = (bot_spent - bot_received).max(0.0);

    // The operation is profitable if bot wallets got back more than they put in
    let profitable = bot_net > 0.0;

    let verdict = if external.unique_wallets == 0 {
        if bot_net >= 0.0 {
            "No external buyers, but bot broke even or profited (unusual)".to_string()
        } else {
            format!(
                "No external buyers — bot lost {:.4} SOL to fees/slippage",
                bot_net.abs()
            )
        }
    } else if profitable {
        format!(
            "PROFITABLE — external buyers injected {:.4} SOL net, bot profited {:.4} SOL",
            external_net_buy, bot_net
        )
    } else {
        format!(
            "UNPROFITABLE — external buyers injected {:.4} SOL net, but overhead was {:.4} SOL (need {:.4} more)",
            external_net_buy, estimated_overhead, (estimated_overhead - external_net_buy).max(0.0)
        )
    };

    Profitability {
        creator_spent_sol: round4(creator_spent),
        creator_received_sol: round4(creator_received),
        creator_net_sol: round4(creator_net),
        bot_total_spent_sol: round4(bot_spent),
        bot_total_received_sol: round4(bot_received),
        bot_net_sol: round4(bot_net),
        external_net_buy_sol: round4(external_net_buy),
        estimated_overhead_sol: round4(estimated_overhead),
        profitable,
        verdict,
    }
}

fn build_assumed_config(
    creator: &CreatorInfo,
    initial: &InitialBuyerInfo,
    staggered: &StaggeredBuyerInfo,
    stag_fund_amounts: &[f64],
) -> AssumedConfig {
    // Detect strategy
    let has_initial = initial.count > 0;
    let has_staggered = staggered.count > 0;
    let has_sells = staggered.total_sells > 0;

    let (strategy, reasoning) = if has_initial && has_staggered && has_sells {
        (3, "Initial buyers + staggered buyers with sells → Strategy 3")
    } else if has_initial && has_staggered && !has_sells {
        (2, "Initial buyers + staggered buyers (buys only) → Strategy 2")
    } else if has_initial && !has_staggered {
        (4, "Initial buyers, no staggered buyers → Strategy 4")
    } else if !has_initial && has_staggered {
        (1, "Staggered buyers only, no initial buyers → Strategy 1")
    } else {
        (0, "Could not determine strategy — no clear bot pattern")
    };

    // Estimate creator_fund_sol: total funding minus the buy amount
    let creator_fund_estimate = match (creator.fund_sol, creator.buy_amount_sol) {
        (Some(fund), Some(buy)) => Some(round4(fund - buy)),
        (Some(fund), None) => Some(round4(fund)),
        _ => None,
    };

    AssumedConfig {
        strategy,
        strategy_reasoning: reasoning.to_string(),
        creator_fund_sol: creator_fund_estimate,
        creator_min_buy_amount: creator.buy_amount_sol.map(round4),
        creator_max_buy_amount: creator.buy_amount_sol.map(round4),
        initial_buyer_count: initial.count,
        initial_buyer_min_buy_amount: initial.min_buy_sol.map(round4),
        initial_buyer_max_buy_amount: initial.max_buy_sol.map(round4),
        buyer_count: staggered.count,
        buyer_fund_sol: avg_f64(stag_fund_amounts).map(round4),
        buy_amount_min_sol: staggered.min_buy_sol.map(round4),
        buy_amount_max_sol: staggered.max_buy_sol.map(round4),
        trade_delay_min_ms: staggered.min_delay_ms,
        trade_delay_max_ms: staggered.max_delay_ms,
        buy_probability: staggered.buy_probability.map(round4),
        sell_pct_min: staggered.sell_pct_min.map(round4),
        sell_pct_max: staggered.sell_pct_max.map(round4),
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn min_f64(values: &[f64]) -> Option<f64> {
    values.iter().copied().reduce(f64::min)
}

fn max_f64(values: &[f64]) -> Option<f64> {
    values.iter().copied().reduce(f64::max)
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
