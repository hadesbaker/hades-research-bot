# hades-research-bot

A Rust-based research tool that analyzes [pump.fun](https://pump.fun) tokens on the Solana blockchain to reverse-engineer the bot configurations that launched them. Built as a companion to [hades-spider-bot](https://github.com/hadesbaker/hades-spider-bot) — feed it any pump.fun token mint address and it produces a full breakdown of the launch strategy, wallet structure, trade timing, and an assumed `config.toml` you can use to replicate or refine your own spider-bot runs.

## How It Works

The research bot performs a multi-phase analysis of on-chain transaction data:

1. **Fetch coin metadata** — queries the pump.fun API for the token's name, ticker, description, and current market cap.
2. **Pull transaction history** — fetches all transaction signatures for the mint address from the Solana RPC, with automatic pagination for tokens with large histories.
3. **Filter to launch window** — isolates the first 15 minutes of the token's life, which captures the critical launch phase (creation, initial buys, staggered trading, and early external activity).
4. **Parse each transaction** — for every transaction in the window, extracts the signer wallet, SOL balance change, token balance change, and classifies the action as create, buy, or sell.
5. **Identify the creator** — the first transaction's signer is the creator wallet.
6. **Trace the primary wallet** — inspects the creator's transaction history to find the wallet that funded it. This is the bot operator's main wallet.
7. **Classify all trading wallets** — for each wallet that traded the token, checks if it was funded by the same primary wallet. If yes, it's a bot wallet. If not, it's an external buyer.
8. **Separate bot phases** — bot wallets are further classified as **initial buyers** (single rapid-fire buy immediately after creation) or **staggered buyers** (multiple trades spread over time with delays).
9. **Compute statistics** — calculates buy/sell counts, SOL volumes, trade delays, sell percentages, and buy probability for each wallet category.
10. **Generate assumed config** — maps all detected parameters back to `hades-spider-bot`'s `config.toml` variables, including strategy detection.
11. **Write results** — outputs a comprehensive JSON analysis file per token to the `results/` directory.

## Architecture

```
src/
  main.rs              Entry point, .env loading, orchestration, summary output
  lib.rs               Module declarations
  rpc.rs               Solana JSON-RPC client (signatures, transactions, pump.fun API)
  types.rs             All data structures (RPC responses, trades, analysis output)
  analysis.rs          Core engine: tx parsing, wallet classification, config estimation

.env                   RPC endpoint and token addresses
results/               Output JSON files (one per analyzed token)
```

## Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) (cargo 1.89+)
- [Git](https://git-scm.com/) (2.42+)
- A Solana mainnet RPC endpoint (no wallet or SOL required — this tool is read-only)

## Setup

### 1. Clone the repository

```bash
git clone https://github.com/hadesbaker/hades-research-bot.git
cd hades-research-bot
```

### 2. Create your `.env` file

Copy the example and fill in your values:

```bash
cp .env.example .env
```

```env
RPC_URL="https://your-rpc-endpoint.example.com"
TOKEN_ADDRESSES="mint1,mint2,mint3"
RUST_LOG="info"
```

### 3. Build and run

```bash
cargo run --release
```

The bot will analyze each token and write results to `results/<mint_address>.json`.

## Environment Variables

| Variable          | Required | Description                                                                                                                                          |
| ----------------- | -------- | ---------------------------------------------------------------------------------------------------------------------------------------------------- |
| `RPC_URL`         | Yes      | Solana mainnet RPC endpoint. Free tiers from [QuikNode](https://www.quicknode.com/), [Helius](https://helius.dev/), or [Alchemy](https://www.alchemy.com/) work fine. The endpoint must support `getSignaturesForAddress` and `getTransaction` with `jsonParsed` encoding. |
| `TOKEN_ADDRESSES` | Yes      | Comma-separated list of pump.fun token mint addresses to analyze. Each token produces its own JSON output file.                                       |
| `RUST_LOG`        | No       | Log verbosity. Options: `trace`, `debug`, `info` (default), `warn`, `error`. Use `debug` for detailed RPC call logging.                              |

## Output Format

Each analyzed token produces a JSON file at `results/<mint_address>.json` with the following structure:

### Top-Level Fields

| Field                        | Type    | Description                                          |
| ---------------------------- | ------- | ---------------------------------------------------- |
| `mint`                       | string  | Token mint address                                   |
| `token_name`                 | string  | Token name from pump.fun                             |
| `token_ticker`               | string  | Token ticker symbol                                  |
| `pumpfun_url`                | string  | Direct link to the token on pump.fun                 |
| `created_at_unix`            | integer | Unix timestamp of the token creation transaction     |
| `analysis_window_minutes`    | integer | Analysis window (15 minutes)                         |
| `total_transactions`         | integer | Total parsed transactions in the window              |
| `total_bot_transactions`     | integer | Transactions from identified bot wallets             |
| `total_external_transactions`| integer | Transactions from external (non-bot) wallets         |
| `primary_wallet`             | string  | The bot operator's main wallet (funding source)      |

### Creator

| Field                      | Type   | Description                                              |
| -------------------------- | ------ | -------------------------------------------------------- |
| `creator.wallet`           | string | Creator wallet public key                                |
| `creator.fund_sol`         | float  | SOL the creator was funded with from the primary wallet  |
| `creator.buy_amount_sol`   | float  | SOL the creator spent buying the token after creation    |
| `creator.buy_delay_after_create_ms` | integer | Milliseconds between creation tx and creator buy tx |

### Initial Buyers

| Field                         | Type   | Description                                            |
| ----------------------------- | ------ | ------------------------------------------------------ |
| `initial_buyers.count`        | integer| Number of initial buyer wallets detected               |
| `initial_buyers.wallets`      | array  | Per-wallet details (pubkey, fund_sol, buy_amount_sol)  |
| `initial_buyers.min_buy_sol`  | float  | Smallest initial buyer purchase                        |
| `initial_buyers.max_buy_sol`  | float  | Largest initial buyer purchase                         |
| `initial_buyers.total_sol`    | float  | Total SOL spent by all initial buyers                  |

### Staggered Buyers

| Field                              | Type   | Description                                                 |
| ---------------------------------- | ------ | ----------------------------------------------------------- |
| `staggered_buyers.count`           | integer| Number of staggered buyer wallets                           |
| `staggered_buyers.total_buys`      | integer| Total buy transactions across all staggered buyers          |
| `staggered_buyers.total_sells`     | integer| Total sell transactions (0 if Strategy 1 or 2)              |
| `staggered_buyers.min_buy_sol`     | float  | Smallest staggered buy amount                               |
| `staggered_buyers.max_buy_sol`     | float  | Largest staggered buy amount                                |
| `staggered_buyers.total_buy_sol`   | float  | Total SOL spent on buys                                     |
| `staggered_buyers.total_sell_sol`  | float  | Total SOL received from sells                               |
| `staggered_buyers.fund_per_wallet_sol` | float | Average SOL each staggered buyer was funded with        |
| `staggered_buyers.delays_ms`       | array  | All inter-trade delays in milliseconds                      |
| `staggered_buyers.min_delay_ms`    | integer| Shortest delay between consecutive trades                   |
| `staggered_buyers.max_delay_ms`    | integer| Longest delay between consecutive trades                    |
| `staggered_buyers.avg_delay_ms`    | integer| Average delay between consecutive trades                    |
| `staggered_buyers.buy_probability` | float  | Ratio of buys to total trades (buys + sells)                |
| `staggered_buyers.sell_percentages`| array  | Percentage of holdings sold in each sell transaction         |
| `staggered_buyers.sell_pct_min`    | float  | Smallest sell percentage observed                           |
| `staggered_buyers.sell_pct_max`    | float  | Largest sell percentage observed                            |

### External Activity

| Field                           | Type    | Description                              |
| ------------------------------- | ------- | ---------------------------------------- |
| `external_activity.unique_wallets` | integer | Number of unique external wallets     |
| `external_activity.total_buys`  | integer | Total external buy transactions          |
| `external_activity.total_sells` | integer | Total external sell transactions         |
| `external_activity.total_buy_sol` | float | Total SOL spent by external buyers       |
| `external_activity.total_sell_sol`| float | Total SOL received by external sellers   |

### Trade Timeline

A chronological array of every trade in the analysis window:

| Field         | Type   | Description                                            |
| ------------- | ------ | ------------------------------------------------------ |
| `timestamp`   | integer| Unix timestamp                                         |
| `signature`   | string | Solana transaction signature                           |
| `wallet`      | string | Wallet that executed the trade                         |
| `wallet_role` | string | `creator`, `initial_buyer`, `staggered_buyer`, or `external` |
| `action`      | string | `Create`, `Buy`, or `Sell`                             |
| `sol_amount`  | float  | SOL spent (buy) or received (sell)                     |
| `token_amount`| float  | Tokens received (buy) or sold (sell)                   |

### Assumed Config

Maps detected parameters directly to `hades-spider-bot`'s `config.toml` variables:

| Field                          | Type    | Description                                              |
| ------------------------------ | ------- | -------------------------------------------------------- |
| `strategy`                     | integer | Detected strategy (1–4). See strategy detection below    |
| `strategy_reasoning`           | string  | Explanation of why this strategy was detected             |
| `creator_fund_sol`             | float   | Estimated `creator_fund_sol`                             |
| `creator_min_buy_amount`       | float   | Estimated `creator_min_buy_amount`                       |
| `creator_max_buy_amount`       | float   | Estimated `creator_max_buy_amount`                       |
| `initial_buyer_count`          | integer | Detected `initial_buyer_count`                           |
| `initial_buyer_min_buy_amount` | float   | Estimated `initial_buyer_min_buy_amount`                 |
| `initial_buyer_max_buy_amount` | float   | Estimated `initial_buyer_max_buy_amount`                 |
| `buyer_count`                  | integer | Detected `buyer_count` (staggered buyers)                |
| `buyer_fund_sol`               | float   | Estimated `buyer_fund_sol` (average funding per buyer)   |
| `buy_amount_min_sol`           | float   | Estimated `buy_amount_min_sol`                           |
| `buy_amount_max_sol`           | float   | Estimated `buy_amount_max_sol`                           |
| `trade_delay_min_ms`           | integer | Estimated `trade_delay_min_ms`                           |
| `trade_delay_max_ms`           | integer | Estimated `trade_delay_max_ms`                           |
| `buy_probability`              | float   | Estimated `buy_probability` (Strategy 3 only)            |
| `sell_pct_min`                 | float   | Estimated `sell_pct_min` (Strategy 3 only)               |
| `sell_pct_max`                 | float   | Estimated `sell_pct_max` (Strategy 3 only)               |

## Strategy Detection

The bot detects which spider-bot strategy was used based on the wallet structure and trading patterns:

| Strategy | Detection Rule                                                    |
| -------- | ----------------------------------------------------------------- |
| 1        | Staggered buyers only, no initial buyers                          |
| 2        | Initial buyers + staggered buyers, buys only (no sells)           |
| 3        | Initial buyers + staggered buyers with both buys and sells        |
| 4        | Initial buyers only, no staggered buyers                          |

## Wallet Classification

The bot uses a two-phase approach to distinguish bot wallets from organic external buyers:

1. **Funding source tracing** — the creator wallet's transaction history is inspected to find the wallet that funded it (the primary/operator wallet). Then every other trading wallet is checked for funding from the same source. Wallets funded by the primary wallet are classified as bot wallets.

2. **Phase detection** — bot wallets are further split into initial buyers (single rapid-fire buy within seconds of creation) and staggered buyers (multiple trades with delays).

Wallets not funded by the primary wallet are classified as external buyers.

## Accuracy Notes

- **SOL amounts** include transaction fees, ATA rent, and pump.fun fees (~1% per trade). Reported buy amounts are slightly higher than the actual `sol_amount` parameter used by the bot.
- **Trade delays** are derived from Solana block timestamps, which have ~400ms slot resolution. Detected delays may differ slightly from the actual configured `trade_delay_min_ms` / `trade_delay_max_ms`.
- **Buy probability** and **sell percentages** are computed from observed trades and may vary from configured values due to randomness in each run.
- **Single-token analysis** — when analyzing a single run of a token, `creator_min_buy_amount` and `creator_max_buy_amount` will be identical (one data point). Analyze multiple tokens from the same operator to estimate the actual range.

## Example

Analyzing a token deployed by `hades-spider-bot` with Strategy 3:

```
$ cargo run --release

════════════════════════════════════════════════════════════════
  Analyzing token 1/1: 7j6VDtVG8FuP2bmzeZuZCFfz8sordp4mUqYDgnMgetcg
════════════════════════════════════════════════════════════════

┌─────────────────────────────────────────────────────────────
│ curds (CURD) — Analysis Complete
├─────────────────────────────────────────────────────────────
│ Mint:              7j6VDtVG8FuP2bmzeZuZCFfz8sordp4mUqYDgnMgetcg
│ Transactions:      72 total (70 bot, 2 external)
│ Primary wallet:    spiderhuf1WwQRuypqCyjora73MqKuff45JFxaU5yC1
│ Creator:           BvC4PxpGpWbH9Ww9YGb7mVswzX97RKtdSmwuFEcxQkUp
│ Creator buy:       0.1987 SOL
│ Initial buyers:    1 (total 0.9070 SOL)
│ Staggered buyers:  10 (41 buys, 24 sells, 4.4247 SOL bought)
│ External wallets:  1 (1 buys = 0.0031 SOL)
├─────────────────────────────────────────────────────────────
│ ASSUMED CONFIG (Strategy 3)
│ Initial buyers + staggered buyers with sells → Strategy 3
│
│ creator_fund_sol           = 0.0313
│ creator_min_buy_amount     = 0.1987
│ creator_max_buy_amount     = 0.1987
│ initial_buyer_count        = 1
│ initial_buyer_min_buy_amount = 0.9070
│ initial_buyer_max_buy_amount = 0.9070
│ buyer_count                = 10
│ buyer_fund_sol             = 0.5000
│ buy_amount_min_sol         = 0.0509
│ buy_amount_max_sol         = 0.1535
│ trade_delay_min_ms         = 2000
│ trade_delay_max_ms         = 9000
│ buy_probability            = 0.63
│ sell_pct_min               = 0.53
│ sell_pct_max               = 1.00
└─────────────────────────────────────────────────────────────
```

## Author

Taki Hades Baker Alyasri

## License

MIT
