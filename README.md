# hades-research-bot

A Rust command-line tool that dissects the launch of any [pump.fun](https://pump.fun) token on Solana. Give it one or more token mint addresses and, for each, it pulls the first 15 minutes of on-chain activity, classifies every transaction as a **create**, **buy**, or **sell**, identifies the creator wallet, and writes a structured JSON report — a creator timeline, headline highlights (volumes, top trades, wallet counts), and statistical distributions of trade sizes, timing, and sell behaviour. It is entirely **read-only**: it holds no wallet, signs nothing, and never trades.

> **Read-only tool.** hades-research-bot only reads public blockchain and API data — it holds no wallet, signs nothing, and cannot trade or move funds. Its analysis is a best-effort heuristic estimate, not ground truth.
>
> **⚠️ Educational use only — see the [Disclaimer](#disclaimer) before relying on its output.**

## Features

- Analyzes any pump.fun token from just its mint address — no wallet, no SOL, no signing
- Reconstructs the **first 15 minutes** of a token's life from on-chain data
- Auto-paginates the full signature history (1,000 per page, up to a 50,000 safety cap)
- Classifies every transaction as **create**, **buy**, or **sell**
- Per-token JSON report with three sections — a **creator** timeline, **highlights**, and **trade stats**
- Highlights: total trades, unique wallets, most active wallet, top 5 buys & sells, buy/sell volume, net inflow
- Trade stats: min / avg / p25 / p75 / max distributions for buy size, sell size, inter-trade delay, and sell-percentage, plus buy/sell probability
- Batch mode — pass many mints, get one report each
- Console summary box printed per token

## Prerequisites

| Tool | Version / type | Why |
| ---- | -------------- | --- |
| Rust | edition 2024 — a recent stable toolchain (1.85+; developed on 1.89) | Required by `edition = "2024"` in `Cargo.toml` |
| A Solana mainnet RPC | HTTP endpoint supporting `getSignaturesForAddress` and `getTransaction` with `jsonParsed` encoding | All transaction data is read from here. A paid provider (QuickNode, Helius, Triton, …) is recommended — large token histories will rate-limit the public endpoint. |

No wallet, keypair, or SOL is required — the tool only reads public data.

## Setup

```bash
# 1. Clone
git clone https://github.com/hadesbaker/hades-research-bot.git
cd hades-research-bot

# 2. Create your .env (git-ignored — copy the template)
cp .env.example .env
# edit .env:
#   RPC_URL=...            your Solana RPC endpoint
#   TOKEN_ADDRESSES=...    comma-separated pump.fun mint addresses
#   RUST_LOG=info          log verbosity (optional)

# 3. Build
cargo build --release
```

## Running

```bash
cargo run --release
```

There are no command-line flags — the tool reads everything from `.env`. It analyzes each mint in `TOKEN_ADDRESSES` in turn, prints a summary box, and writes a report to `results/<mint>.json`. The `results/` directory is created automatically and is git-ignored.

### Logging

Verbosity is controlled by `RUST_LOG` (read from `.env` or the shell environment): `trace`, `debug`, `info` (default), `warn`, or `error`. Use `RUST_LOG=debug` to see each RPC page fetch and per-transaction progress. To keep a log file, redirect output:

```bash
cargo run --release 2>&1 | tee run.log
```

## Configuration

All configuration is via `.env` (git-ignored — copy it from `.env.example`):

| Variable | Required | Purpose |
| -------- | -------- | ------- |
| `RPC_URL` | Yes | Solana RPC endpoint for `getSignaturesForAddress` and `getTransaction` (`jsonParsed`) |
| `TOKEN_ADDRESSES` | Yes | Comma-separated pump.fun token mint addresses to analyze; each produces its own report |
| `RUST_LOG` | No | Log verbosity — `trace` / `debug` / `info` (default) / `warn` / `error` |

## How it works

For each mint in `TOKEN_ADDRESSES`:

1. **Fetch metadata** — queries the pump.fun API (`frontend-api-v3.pump.fun`) for the token's name and ticker; falls back to `unknown` / `???` if it is unavailable.
2. **Fetch signature history** — pulls every transaction signature for the mint via `getSignaturesForAddress`, paginating 1,000 at a time (safety cap: 50,000).
3. **Define the launch window** — the oldest successful transaction is taken as the token's creation; the analysis window is the **first 15 minutes** from that timestamp.
4. **Fetch & parse transactions** — each in-window transaction is fetched (`getTransaction`, `jsonParsed`) and reduced to a trade: the signer wallet, its SOL balance change, and its token balance change. A 50 ms pause between calls keeps RPC load polite.
5. **Classify** — the first transaction is the **create**; after that, a rise in the signer's token balance is a **buy** and a fall is a **sell**.
6. **Identify the creator** — the signer of that first transaction.
7. **Aggregate** — builds the creator timeline, the highlights, and the statistical trade distributions (over buys and sells only — the create is excluded).
8. **Write the report** — writes `results/<mint>.json` and prints a summary box to the console.

## Output

Each analyzed token produces `results/<mint>.json` with three sections.

### Top level

| Field | Type | Description |
| ----- | ---- | ----------- |
| `mint` | string | Token mint address |
| `token_name` / `token_ticker` | string | Name and ticker from pump.fun (`unknown` / `???` if unavailable) |
| `pumpfun_url` | string | Direct pump.fun link |
| `created_at_unix` | integer | Unix timestamp of the creation transaction |
| `analysis_window_minutes` | integer | Analysis window — always `15` |
| `creator` / `highlights` / `trade_stats` | object | See below |

### `creator`

| Field | Type | Description |
| ----- | ---- | ----------- |
| `wallet` | string | Creator wallet (signer of the first transaction) |
| `trades[]` | array | The creator's own in-window transactions — each has `timestamp`, `signature`, `action` (`Create` / `Buy` / `Sell`), `sol_amount`, `token_amount`, `ms_from_create` |

### `highlights`

Computed over **buy and sell transactions only** — the create is excluded.

| Field | Type | Description |
| ----- | ---- | ----------- |
| `total_trades` / `total_buys` / `total_sells` | integer | Trade counts within the window |
| `unique_wallets` | integer | Distinct wallets that bought or sold |
| `most_active_wallet` | object | `{ address, trade_count }` — the wallet with the most trades |
| `top_buys` / `top_sells` | array | Up to 5 largest buys / sells by SOL — each `{ wallet, sol_amount, timestamp }` |
| `total_buy_volume_sol` / `total_sell_volume_sol` | float | Total SOL bought / sold |
| `net_inflow_sol` | float | `total_buy_volume_sol − total_sell_volume_sol` |

### `trade_stats`

| Field | Type | Description |
| ----- | ---- | ----------- |
| `buys` / `sells` | object | Distribution of buy / sell sizes in SOL — `count`, `min`, `max`, `avg`, `p25`, `p75`, `total_sol` |
| `delays_ms` | object | Distribution of gaps between consecutive trades, in milliseconds — `min`, `max`, `avg`, `p25`, `p75` |
| `sell_percentages` | object | Distribution of the fraction of holdings sold per sell — `min`, `max`, `avg`, `p25`, `p75` |
| `buy_probability` / `sell_probability` | float | Share of all trades that were buys / sells |

### Accuracy notes

- **The creator** is identified purely as the signer of the first in-window transaction.
- **SOL amounts** are the signer's net balance change, so they include network fees, ATA rent, and pump.fun fees — buy/sell sizes therefore read slightly higher than the raw trade amount.
- **Inter-trade delays** are derived from Solana block timestamps, which have one-second resolution — every `delays_ms` value is a multiple of 1000.
- **The 15-minute window** is fixed (a constant in `analysis.rs`).
- A transaction in which the signer's token balance does not change is classified `Unknown` and is excluded from the highlights and stats.

## Example

Running the tool against a single token:

```
$ cargo run --release

════════════════════════════════════════════════════════════════
  Analyzing token 1/1: J6gefFyTPhWWRdu2LhMy8PFmAuCftgTJ5pbMym36pump
════════════════════════════════════════════════════════════════

┌─────────────────────────────────────────────────────────────
│ low cap coin (lowcap) — Analysis Complete
├─────────────────────────────────────────────────────────────
│ Mint:              J6gefFyTPhWWRdu2LhMy8PFmAuCftgTJ5pbMym36pump
│ Creator:           Gt4bHbddGatddFcGnGL9hRCZcnx4LYBBmYbz4QatK5Cw
│ Creator txs:       2
├─────────────────────────────────────────────────────────────
│ HIGHLIGHTS
│ Total trades:      626 (420 buys, 206 sells)
│ Unique wallets:    349
│ Most active:       BcvQbvzbcecD1q2i9Ras3sM5JXYXz4z1ZnkHg9DqFoo1 (13 trades)
│ Buy volume:        246.2434 SOL
│ Sell volume:       97.8452 SOL
│ Net inflow:        +148.3982 SOL
│ Top buy:           26.0021 SOL (DCy31D51Uy9xo3HaToYVfuakHd7e5jbHH3oVCSxDRmFu)
│ Top sell:          3.6357 SOL (9muvqhKeDuHNBDP5hrLmDZSwhzgHbd9nUsgaPv8qL42v)
├─────────────────────────────────────────────────────────────
│ TRADE STATS
│ Buy probability:   0.6709
│ Sell probability:  0.3291
│
│ Buy SOL:    min 0.0000  p25 0.1215  avg 0.5863  p75 0.5756  max 26.0021
│ Sell SOL:   min 0.0000  p25 0.0633  avg 0.4750  p75 0.6344  max 3.6357
│ Delays ms:  min 0  p25 0  avg 1440  p75 2000  max 8000
│ Sell %:     min 0.0300  p25 0.2500  avg 0.6670  p75 1.0000  max 1.0000
└─────────────────────────────────────────────────────────────
```

The full structured report is written to `results/J6gefFyTPhWWRdu2LhMy8PFmAuCftgTJ5pbMym36pump.json`.

## Project structure

```
src/
  main.rs       entry point: .env loading, per-token orchestration, console summary
  lib.rs        module declarations
  rpc.rs        Solana JSON-RPC client + pump.fun metadata API
  types.rs      all data structures — RPC responses, parsed trades, the JSON report
  analysis.rs   analysis engine: signature fetch, transaction parsing, classification, stats
```

## Disclaimer

**This software is provided for educational and informational purposes only.**

- **Not financial advice.** Nothing in this repository — code, comments, documentation, output, or examples — constitutes financial, investment, trading, legal, or tax advice. It is a technical demonstration of on-chain data analysis.
- **Read-only tool.** This software only reads public on-chain and API data. It holds no wallet, no private keys, and no funds, and it never signs or submits a transaction — it cannot place trades or move assets.
- **Analysis is heuristic.** The bot classifies and aggregates transactions with best-effort heuristics. Its output is an estimate derived from public data, may be incomplete or incorrect, and must not be relied upon as fact.
- **No warranty.** This software is provided "AS IS", without warranty of any kind, express or implied. It may contain bugs, may misclassify transactions, and may produce inaccurate results — including through software defects, RPC or API failures, or rate limiting.
- **No liability.** To the maximum extent permitted by law, the author(s) and contributors shall not be liable for any claim, damages, or other liability arising from or in connection with the use of, or inability to use, this software, or any decision made on the basis of its output.
- **You are solely responsible** for how you use this software and its output, and for complying with all laws, regulations, and third-party terms of service (including those of pump.fun and your RPC provider) applicable in your jurisdiction.

By using, running, modifying, or distributing this software, you acknowledge that you have read and understood this disclaimer and accept full responsibility for the outcomes.

## Author

**Taki Hades Baker Alyasri**

## License

MIT — see the [Disclaimer](#disclaimer) above. The MIT license's "AS IS", no-warranty, and no-liability terms apply to all use of this software.
