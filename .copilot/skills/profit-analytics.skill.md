---
name: "Profit Analytics"
description: "Kârlılık analizi, trend tespiti ve strateji optimizasyonu"
---

# 📊 PROFIT ANALYTICS SKILL

> **Amaç:** Arbitraj kârlılığını analiz et, trendleri tespit et ve strateji optimizasyonu öner.

## KULLANIM SENARYOLARI

### Senaryo 1: Kârlılık Raporu
```
"Son 24 saatlik kâr analizini göster"
"Haftalık performans raporu"
```

### Senaryo 2: Pool Performansı
```
"En kârlı havuzları listele"
"Düşük performanslı routeları bul"
```

### Senaryo 3: Strateji Optimizasyonu
```
"minProfit threshold'u optimize et"
"Gas stratejisi öner"
```

## METRİK TANIMLARI

### Temel Metrikler

| Metrik | Formül | Açıklama |
|--------|--------|----------|
| Gross Profit | output - input | Ham kâr |
| Gas Cost | gas_used × gas_price | İşlem maliyeti |
| Net Profit | gross_profit - gas_cost | Net kâr |
| ROI | net_profit / input × 100 | Yatırım getirisi |
| Win Rate | successful_arbs / total_attempts × 100 | Başarı oranı |
| Avg Profit | total_profit / successful_arbs | Ortalama kâr |

### Gelişmiş Metrikler

| Metrik | Formül | Açıklama |
|--------|--------|----------|
| Sharpe Ratio | (avg_return - risk_free) / std_dev | Risk-ayarlı getiri |
| Max Drawdown | max(peak - trough) | Maksimum düşüş |
| Profit Factor | gross_profit / gross_loss | Kâr/zarar oranı |
| Hit Ratio | profitable_trades / total_trades | Kârlı işlem oranı |

## VERİ KAYNAKLARI

### Shadow Analytics Log
```jsonl
// shadow_analytics.jsonl
{"timestamp":"2026-04-01T20:30:00Z","event":"arb_attempt","pool_a":"0x...","pool_b":"0x...","input_wei":"1000000000000000000","output_wei":"1005234000000000000","gas_used":245000,"gas_price_gwei":0.001,"status":"success"}
{"timestamp":"2026-04-01T20:30:05Z","event":"arb_attempt","pool_a":"0x...","pool_b":"0x...","input_wei":"500000000000000000","output_wei":"0","gas_used":150000,"gas_price_gwei":0.001,"status":"reverted","reason":"InsufficientProfit"}
```

### On-Chain Data
```rust
// TX receipts
struct ArbitrageReceipt {
    tx_hash: TxHash,
    block_number: u64,
    gas_used: u64,
    effective_gas_price: u128,
    status: bool,
    logs: Vec<Log>,
}
```

## ANALİZ PIPELINE

```
┌─────────────────────────────────────────────────────────────┐
│                   ANALYTICS PIPELINE                        │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  1. DATA COLLECTION                                         │
│     ├── Parse shadow_analytics.jsonl                        │
│     ├── Fetch on-chain TX receipts                          │
│     └── Aggregate by time period                            │
│                                                             │
│  2. METRIC CALCULATION                                      │
│     ├── Calculate per-TX metrics                            │
│     ├── Aggregate by pool, route, time                      │
│     └── Compute derived metrics                             │
│                                                             │
│  3. TREND ANALYSIS                                          │
│     ├── Rolling averages (1h, 24h, 7d)                      │
│     ├── Variance analysis                                   │
│     └── Anomaly detection                                   │
│                                                             │
│  4. OPTIMIZATION SUGGESTIONS                                │
│     ├── minProfit tuning                                    │
│     ├── Gas strategy                                        │
│     └── Pool selection                                      │
│                                                             │
│  5. REPORTING                                               │
│     ├── Generate report                                     │
│     └── Send to Telegram (if configured)                    │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

## DASHBOARD TEMPLATE

```
═══════════════════════════════════════════════════════════════
📊 PROFIT ANALYTICS DASHBOARD
═══════════════════════════════════════════════════════════════
Period: Last 24 Hours
Generated: 2026-04-01T21:00:00Z

💰 SUMMARY:
┌─────────────────────────────────────────────────────────────┐
│ Total Attempts:     156                                     │
│ Successful:         142 (91.0%)                             │
│ Reverted:           14 (9.0%)                               │
│                                                             │
│ Gross Profit:       0.847 ETH ($2,541)                      │
│ Gas Cost:           0.023 ETH ($69)                         │
│ Net Profit:         0.824 ETH ($2,472)                      │
│ ROI:                0.58%                                   │
│ Avg Profit/Trade:   0.0058 ETH ($17.41)                     │
└─────────────────────────────────────────────────────────────┘

📈 HOURLY TREND:
  Profit (ETH)
  0.08│    ╭─╮
  0.06│ ╭──╯ ╰─╮  ╭───╮
  0.04│─╯      ╰──╯   ╰─────╮
  0.02│                     ╰──
  0.00└────────────────────────
      00  04  08  12  16  20  24h

🏆 TOP PERFORMING ROUTES:
┌─────────────────────────────────────────────────────────────┐
│ ROUTE                    │ TRADES │ NET PROFIT │ AVG PROFIT │
├──────────────────────────┼────────┼────────────┼────────────┤
│ WETH→USDC→WETH (UniV3)   │ 47     │ 0.312 ETH  │ 0.0066 ETH │
│ WETH→DAI→WETH (Aero)     │ 38     │ 0.234 ETH  │ 0.0062 ETH │
│ cbETH→WETH→cbETH (UniV3) │ 23     │ 0.145 ETH  │ 0.0063 ETH │
│ USDC→WETH→USDC (Sushi)   │ 19     │ 0.089 ETH  │ 0.0047 ETH │
│ AERO→WETH→AERO (Aero)    │ 15     │ 0.044 ETH  │ 0.0029 ETH │
└─────────────────────────────────────────────────────────────┘

❌ REVERT ANALYSIS:
┌─────────────────────────────────────────────────────────────┐
│ REASON                   │ COUNT │ %      │ ACTION         │
├──────────────────────────┼───────┼────────┼────────────────┤
│ InsufficientProfit       │ 8     │ 57.1%  │ Lower minProfit│
│ DeadlineExpired          │ 4     │ 28.6%  │ Increase deadline │
│ PoolNotWhitelisted       │ 2     │ 14.3%  │ Add to whitelist │
└─────────────────────────────────────────────────────────────┘

⛽ GAS ANALYSIS:
├── Avg Gas Used:     198,000
├── Avg Gas Price:    0.001 gwei (Base L2)
├── Gas Efficiency:   98.5% (vs estimate)
└── L1 Data Fee:      ~$0.02 per TX

🔧 OPTIMIZATION SUGGESTIONS:
1. minProfit = 0.003 ETH → 0.0025 ETH (catch 8 more arbs)
2. Add pool 0x1234... to whitelist (2 missed opportunities)
3. Consider IPC connection (reduce sync latency)

═══════════════════════════════════════════════════════════════
```

## KÂR OPTİMİZASYONU

### minProfit Tuning

```rust
/// Find optimal minProfit based on historical data
fn optimize_min_profit(trades: &[TradeRecord]) -> U256 {
    // Sort by profit
    let mut profits: Vec<U256> = trades
        .iter()
        .filter(|t| t.status == Status::Success)
        .map(|t| t.net_profit)
        .collect();
    profits.sort();

    // Find 5th percentile (exclude outliers)
    let idx = profits.len() * 5 / 100;
    let p5_profit = profits[idx];

    // Set minProfit at 80% of 5th percentile
    // This catches more opportunities while maintaining safety
    p5_profit * U256::from(80) / U256::from(100)
}
```

### Gas Strategy

```rust
/// Determine optimal gas price
fn optimize_gas_price(
    opportunity: &Opportunity,
    base_fee: u128,
) -> u128 {
    let profit_wei = opportunity.expected_profit;

    // Max gas willing to spend: 20% of profit
    let max_gas_budget = profit_wei * 20 / 100;
    let estimated_gas = 250_000u128;

    let max_gas_price = max_gas_budget / estimated_gas;

    // Use minimum of: max_gas_price, 2x base_fee
    max_gas_price.min(base_fee * 2)
}
```

### Pool Selection

```rust
/// Score pools for priority
fn prioritize_pools(pools: &[PoolData]) -> Vec<(Address, u8)> {
    pools.iter()
        .map(|p| {
            let score = calculate_profitability_score(p);
            (p.address, score)
        })
        .filter(|(_, score)| *score >= 50)
        .sorted_by(|a, b| b.1.cmp(&a.1))
        .collect()
}

fn calculate_profitability_score(pool: &PoolData) -> u8 {
    let mut score = 0u32;

    // Historical profit score (0-40)
    score += (pool.total_profit_wei / U256::from(1e16 as u64))
        .min(U256::from(40))
        .to::<u32>();

    // Success rate score (0-30)
    let success_rate = pool.successful_arbs as f64 / pool.total_attempts as f64;
    score += (success_rate * 30.0) as u32;

    // Frequency score (0-30)
    let arbs_per_day = pool.total_arbs as f64 / pool.days_active as f64;
    score += (arbs_per_day * 10.0).min(30.0) as u32;

    score.min(100) as u8
}
```

## ALERT THRESHOLDS

```rust
struct AlertConfig {
    // Performance alerts
    profit_drop_threshold: f64,      // -20% from 24h avg
    revert_rate_threshold: f64,      // > 15%
    gas_spike_threshold: f64,        // > 50% from avg

    // Opportunity alerts
    large_opportunity_eth: f64,      // > 0.1 ETH
    high_volume_period: bool,        // Unusual activity

    // System alerts
    latency_spike_ms: u64,           // > 150ms
    sync_stale_ms: u64,              // > 2000ms
}
```

## TELEGRAM REPORT

```rust
async fn send_daily_report(metrics: &DailyMetrics) -> Result<()> {
    let message = format!(
        "📊 *Daily Arbitrage Report*\n\
        \n\
        💰 *Net Profit:* {:.4} ETH (${:.2})\n\
        📈 *Win Rate:* {:.1}%\n\
        🎯 *Trades:* {} successful / {} total\n\
        ⛽ *Gas Spent:* {:.4} ETH\n\
        \n\
        🏆 *Top Route:* {}\n\
        ❌ *Main Revert:* {}\n\
        \n\
        _Generated: {}_",
        metrics.net_profit_eth,
        metrics.net_profit_usd,
        metrics.win_rate * 100.0,
        metrics.successful_trades,
        metrics.total_trades,
        metrics.gas_spent_eth,
        metrics.top_route,
        metrics.main_revert_reason,
        chrono::Utc::now().format("%Y-%m-%d %H:%M UTC"),
    );

    telegram_send(&message).await
}
```

## ÇIKIŞ RAPORU

```
═══════════════════════════════════════════════════════════════
📊 PROFIT ANALYTICS REPORT
═══════════════════════════════════════════════════════════════
Period: [start] to [end]
Generated: [timestamp]

📈 KEY METRICS:
├── Net Profit:      X.XXX ETH ($Y,YYY)
├── Win Rate:        XX.X%
├── Avg Profit:      0.00XX ETH
├── Profit Factor:   X.XX
└── Sharpe Ratio:    X.XX

🔝 TOP ROUTES: [list]
❌ REVERT REASONS: [analysis]
🔧 OPTIMIZATIONS: [suggestions]

═══════════════════════════════════════════════════════════════
```

---

*"Veri, kârın pusulasıdır."*
