use std::path::PathBuf;
use tracing::{error, info};
use tracing_subscriber::{fmt, EnvFilter};

use hades_research_bot::analysis;
use hades_research_bot::rpc::SolanaRpc;
use hades_research_bot::types::*;

const RESULTS_DIR: &str = "results";

#[tokio::main]
async fn main() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
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

    let rpc_url =
        std::env::var("RPC_URL").map_err(|_| anyhow::anyhow!("RPC_URL not set in .env"))?;

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
                let output_path = results_dir.join(format!("{}.json", mint));
                let json = serde_json::to_string_pretty(&analysis)?;
                std::fs::write(&output_path, &json)?;

                print_summary(&analysis);
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

fn print_summary(a: &TokenAnalysis) {
    let h = &a.highlights;
    let s = &a.trade_stats;

    println!();
    println!("┌─────────────────────────────────────────────────────────────");
    println!("│ {} ({}) — Analysis Complete", a.token_name, a.token_ticker);
    println!("├─────────────────────────────────────────────────────────────");
    println!("│ Mint:              {}", a.mint);
    println!("│ Creator:           {}", a.creator.wallet);
    println!("│ Creator txs:       {}", a.creator.trades.len());
    println!("├─────────────────────────────────────────────────────────────");
    println!("│ HIGHLIGHTS");
    println!(
        "│ Total trades:      {} ({} buys, {} sells)",
        h.total_trades, h.total_buys, h.total_sells
    );
    println!("│ Unique wallets:    {}", h.unique_wallets);
    if let Some(ref m) = h.most_active_wallet {
        println!("│ Most active:       {} ({} trades)", m.address, m.trade_count);
    }
    println!("│ Buy volume:        {:.4} SOL", h.total_buy_volume_sol);
    println!("│ Sell volume:       {:.4} SOL", h.total_sell_volume_sol);
    println!("│ Net inflow:        {:+.4} SOL", h.net_inflow_sol);
    if let Some(top) = h.top_buys.first() {
        println!("│ Top buy:           {:.4} SOL ({})", top.sol_amount, top.wallet);
    }
    if let Some(top) = h.top_sells.first() {
        println!("│ Top sell:          {:.4} SOL ({})", top.sol_amount, top.wallet);
    }
    println!("├─────────────────────────────────────────────────────────────");
    println!("│ TRADE STATS");
    if let Some(p) = s.buy_probability {
        println!("│ Buy probability:   {:.4}", p);
    }
    if let Some(p) = s.sell_probability {
        println!("│ Sell probability:  {:.4}", p);
    }
    println!("│");
    println!(
        "│ Buy SOL:    min {}  p25 {}  avg {}  p75 {}  max {}",
        fmt_f64(s.buys.min),
        fmt_f64(s.buys.p25),
        fmt_f64(s.buys.avg),
        fmt_f64(s.buys.p75),
        fmt_f64(s.buys.max)
    );
    println!(
        "│ Sell SOL:   min {}  p25 {}  avg {}  p75 {}  max {}",
        fmt_f64(s.sells.min),
        fmt_f64(s.sells.p25),
        fmt_f64(s.sells.avg),
        fmt_f64(s.sells.p75),
        fmt_f64(s.sells.max)
    );
    println!(
        "│ Delays ms:  min {}  p25 {}  avg {}  p75 {}  max {}",
        fmt_i64(s.delays_ms.min),
        fmt_i64(s.delays_ms.p25),
        fmt_i64(s.delays_ms.avg),
        fmt_i64(s.delays_ms.p75),
        fmt_i64(s.delays_ms.max)
    );
    println!(
        "│ Sell %:     min {}  p25 {}  avg {}  p75 {}  max {}",
        fmt_f64(s.sell_percentages.min),
        fmt_f64(s.sell_percentages.p25),
        fmt_f64(s.sell_percentages.avg),
        fmt_f64(s.sell_percentages.p75),
        fmt_f64(s.sell_percentages.max)
    );
    println!("└─────────────────────────────────────────────────────────────");
    println!();
}

fn fmt_f64(v: Option<f64>) -> String {
    match v {
        Some(x) => format!("{:.4}", x),
        None => "-".into(),
    }
}

fn fmt_i64(v: Option<i64>) -> String {
    match v {
        Some(x) => x.to_string(),
        None => "-".into(),
    }
}
