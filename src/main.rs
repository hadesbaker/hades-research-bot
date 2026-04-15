use std::path::PathBuf;
use tracing::{error, info};
use tracing_subscriber::{fmt, EnvFilter};

use hades_research_bot::analysis;
use hades_research_bot::rpc::SolanaRpc;

const RESULTS_DIR: &str = "results";

#[tokio::main]
async fn main() {
    // Init logging
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));
    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_file(false)
        .with_line_number(false)
        .init();

    if let Err(e) = run().await {
        error!(%e, "Research bot failed");
        std::process::exit(1);
    }
}

async fn run() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    // Load config from .env
    let rpc_url = std::env::var("RPC_URL")
        .map_err(|_| anyhow::anyhow!("RPC_URL not set in .env"))?;

    let token_addresses_raw = std::env::var("TOKEN_ADDRESSES")
        .map_err(|_| anyhow::anyhow!("TOKEN_ADDRESSES not set in .env"))?;

    let mints: Vec<&str> = token_addresses_raw
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    if mints.is_empty() {
        anyhow::bail!("TOKEN_ADDRESSES is empty — provide comma-separated mint addresses");
    }

    info!(count = mints.len(), "Tokens to analyze");

    // Create results directory
    let results_dir = PathBuf::from(RESULTS_DIR);
    std::fs::create_dir_all(&results_dir)?;

    let rpc = SolanaRpc::new(&rpc_url);

    for (i, mint) in mints.iter().enumerate() {
        println!();
        println!("════════════════════════════════════════════════════════════════");
        println!("  Analyzing token {}/{}: {}", i + 1, mints.len(), mint);
        println!("════════════════════════════════════════════════════════════════");
        println!();

        match analysis::analyze_token(&rpc, mint).await {
            Ok(analysis) => {
                // Write to results/<mint>.json
                let output_path = results_dir.join(format!("{}.json", mint));
                let json = serde_json::to_string_pretty(&analysis)?;
                std::fs::write(&output_path, &json)?;

                // Print summary
                println!();
                println!("┌─────────────────────────────────────────────────────────────");
                println!("│ {} ({}) — Analysis Complete", analysis.token_name, analysis.token_ticker);
                println!("├─────────────────────────────────────────────────────────────");
                println!("│ Mint:              {}", analysis.mint);
                println!("│ Transactions:      {} total ({} bot, {} external)",
                    analysis.total_transactions,
                    analysis.total_bot_transactions,
                    analysis.total_external_transactions,
                );
                if let Some(ref pw) = analysis.primary_wallet {
                    println!("│ Primary wallet:    {}", pw);
                }
                println!("│ Creator:           {}", analysis.creator.wallet);
                if let Some(buy) = analysis.creator.buy_amount_sol {
                    println!("│ Creator buy:       {:.4} SOL", buy);
                }
                println!("│ Initial buyers:    {} (total {:.4} SOL)",
                    analysis.initial_buyers.count,
                    analysis.initial_buyers.total_sol,
                );
                println!("│ Staggered buyers:  {} ({} buys, {} sells, {:.4} SOL bought)",
                    analysis.staggered_buyers.count,
                    analysis.staggered_buyers.total_buys,
                    analysis.staggered_buyers.total_sells,
                    analysis.staggered_buyers.total_buy_sol,
                );
                println!("│ External wallets:  {} ({} buys = {:.4} SOL)",
                    analysis.external_activity.unique_wallets,
                    analysis.external_activity.total_buys,
                    analysis.external_activity.total_buy_sol,
                );
                println!("├─────────────────────────────────────────────────────────────");
                println!("│ ASSUMED CONFIG (Strategy {})", analysis.assumed_config.strategy);
                println!("│ {}", analysis.assumed_config.strategy_reasoning);
                println!("│");

                let cfg = &analysis.assumed_config;
                if let Some(v) = cfg.creator_fund_sol {
                    println!("│ creator_fund_sol           = {:.4}", v);
                }
                if let Some(v) = cfg.creator_min_buy_amount {
                    println!("│ creator_min_buy_amount     = {:.4}", v);
                }
                if let Some(v) = cfg.creator_max_buy_amount {
                    println!("│ creator_max_buy_amount     = {:.4}", v);
                }
                println!("│ initial_buyer_count        = {}", cfg.initial_buyer_count);
                if let Some(v) = cfg.initial_buyer_min_buy_amount {
                    println!("│ initial_buyer_min_buy_amount = {:.4}", v);
                }
                if let Some(v) = cfg.initial_buyer_max_buy_amount {
                    println!("│ initial_buyer_max_buy_amount = {:.4}", v);
                }
                println!("│ buyer_count                = {}", cfg.buyer_count);
                if let Some(v) = cfg.buyer_fund_sol {
                    println!("│ buyer_fund_sol             = {:.4}", v);
                }
                if let Some(v) = cfg.buy_amount_min_sol {
                    println!("│ buy_amount_min_sol         = {:.4}", v);
                }
                if let Some(v) = cfg.buy_amount_max_sol {
                    println!("│ buy_amount_max_sol         = {:.4}", v);
                }
                if let Some(v) = cfg.trade_delay_min_ms {
                    println!("│ trade_delay_min_ms         = {}", v);
                }
                if let Some(v) = cfg.trade_delay_max_ms {
                    println!("│ trade_delay_max_ms         = {}", v);
                }
                if let Some(v) = cfg.buy_probability {
                    println!("│ buy_probability            = {:.2}", v);
                }
                if let Some(v) = cfg.sell_pct_min {
                    println!("│ sell_pct_min               = {:.2}", v);
                }
                if let Some(v) = cfg.sell_pct_max {
                    println!("│ sell_pct_max               = {:.2}", v);
                }
                println!("└─────────────────────────────────────────────────────────────");
                println!();
                info!(path = %output_path.display(), "Results written");
            }
            Err(e) => {
                error!(mint, %e, "Analysis failed");
            }
        }
    }

    info!("All analyses complete");
    Ok(())
}
