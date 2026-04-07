---
name: "Pool Discovery"
description: "Yeni arbitraj havuzlarını keşfet, analiz et ve whitelist'e ekle"
---

# 🔍 POOL DISCOVERY SKILL

> **Amaç:** Yeni arbitraj fırsatı taşıyan havuzları otonom keşfet, risk analizi yap ve whitelist'e ekle.

## KULLANIM SENARYOLARI

### Senaryo 1: Yeni Pool Keşfi
```
"Base'deki yeni UniV3 havuzlarını keşfet"
"Yüksek hacimli Aerodrome poollarını bul"
```

### Senaryo 2: Pool Analizi
```
"Bu pool'u analiz et: 0x..."
"Pool arbitraj potansiyelini değerlendir"
```

### Senaryo 3: Whitelist Yönetimi
```
"Whitelist'e pool ekle"
"Düşük performanslı poolları kaldır"
```

## POOL DISCOVERY PIPELINE

```
┌─────────────────────────────────────────────────────────────┐
│                   POOL DISCOVERY PIPELINE                   │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  STAGE 1: SOURCE DISCOVERY                                  │
│  ├── Factory PoolCreated events                             │
│  ├── Subgraph queries                                       │
│  └── On-chain pool enumeration                              │
│                                                             │
│  STAGE 2: INITIAL FILTER                                    │
│  ├── TVL > $50k                                             │
│  ├── 24h volume > $10k                                      │
│  └── Token pair in allowed list                             │
│                                                             │
│  STAGE 3: DEEP ANALYSIS                                     │
│  ├── Liquidity distribution                                 │
│  ├── Historical volatility                                  │
│  └── Arbitrage frequency                                    │
│                                                             │
│  STAGE 4: SCORING                                           │
│  ├── Calculate composite score (0-100)                      │
│  └── Rank by potential                                      │
│                                                             │
│  STAGE 5: WHITELIST PROPOSAL                                │
│  ├── Top N pools for review                                 │
│  └── Add to contract whitelist                              │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

## DESTEKLENEN DEX'LER (Base Network)

| DEX | Type | Factory | Discovery Method |
|-----|------|---------|------------------|
| Uniswap V3 | Concentrated | 0x33128a8fC17869897dcE68Ed026d694621f6FDfD | Factory events |
| Aerodrome | CL | 0x5e7BB104d84c7CB9B682AaC2F3d509f5F406809A | Factory events |
| SushiSwap V3 | Concentrated | 0x7a5c73FE2e6d0BbBbFaE1B58B0b61e2BCf63A0e7 | Factory events |
| PancakeSwap V3 | Concentrated | 0x0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865 | Factory events |
| BaseSwap | V2 | 0xFDa619b6d20975be80A10332dD3c9DCCea29E26B | Factory events |

## POOL SCORING ALGORITHM

```rust
/// Pool scoring: 0-100
fn calculate_pool_score(pool: &PoolData) -> u8 {
    let mut score = 0u32;

    // TVL Score (0-30)
    score += match pool.tvl_usd {
        tvl if tvl > 10_000_000.0 => 30,
        tvl if tvl > 1_000_000.0 => 25,
        tvl if tvl > 100_000.0 => 20,
        tvl if tvl > 50_000.0 => 15,
        _ => 5,
    };

    // Volume Score (0-25)
    score += match pool.volume_24h_usd {
        vol if vol > 1_000_000.0 => 25,
        vol if vol > 100_000.0 => 20,
        vol if vol > 10_000.0 => 15,
        _ => 5,
    };

    // Fee Tier Score (0-20)
    // Lower fees = more arbitrage opportunities
    score += match pool.fee_bps {
        1 => 20,   // 0.01%
        5 => 18,   // 0.05%
        30 => 15,  // 0.30%
        100 => 10, // 1.00%
        _ => 5,
    };

    // Volatility Score (0-15)
    // Higher volatility = more opportunities
    score += (pool.volatility_24h * 100.0).min(15.0) as u32;

    // Counterparty Score (0-10)
    // Having good counterparty pools increases score
    score += pool.counterparty_count.min(10) as u32;

    score.min(100) as u8
}
```

## POOL DATA STRUCTURE

```rust
#[derive(Debug, Clone)]
pub struct PoolData {
    // Identity
    pub address: Address,
    pub dex_type: DexType,
    pub token0: Address,
    pub token1: Address,
    pub fee_bps: u16,

    // Metrics
    pub tvl_usd: f64,
    pub volume_24h_usd: f64,
    pub volatility_24h: f64,
    pub liquidity: U256,
    pub sqrt_price_x96: U256,
    pub tick: i32,

    // Scoring
    pub score: u8,
    pub counterparty_count: u8,
    pub last_arb_profit_wei: U256,

    // Metadata
    pub created_block: u64,
    pub last_sync: Instant,
}
```

## DISCOVERY QUERIES

### Factory Event Query
```rust
// Bot/src/pool_discovery.rs
let filter = factory.PoolCreated_filter()
    .from_block(start_block)
    .to_block(end_block);

let events = filter.query().await?;
for event in events {
    let pool = PoolData {
        address: event.pool,
        token0: event.token0,
        token1: event.token1,
        fee_bps: event.fee.try_into()?,
        // ...
    };
    process_new_pool(pool).await?;
}
```

### Subgraph Query
```graphql
# TheGraph query for high-volume pools
{
  pools(
    first: 100
    orderBy: volumeUSD
    orderDirection: desc
    where: { volumeUSD_gt: "10000" }
  ) {
    id
    token0 { id symbol }
    token1 { id symbol }
    feeTier
    volumeUSD
    totalValueLockedUSD
    txCount
  }
}
```

## WHITELIST MANAGEMENT

### Add Pool to Whitelist
```rust
// Bot/src/discovery_engine.rs
async fn add_pool_to_whitelist(pools: &[Address]) -> Result<()> {
    let calldata = executorBatchAddPoolsCall {
        pools: pools.to_vec(),
    }.abi_encode();

    let tx = TransactionRequest::default()
        .to(contract_address)
        .input(calldata.into());

    let receipt = provider.send_transaction(tx).await?;
    info!("Added {} pools to whitelist: {:?}", pools.len(), receipt.transaction_hash);
    Ok(())
}
```

### Remove Low-Performing Pool
```rust
// Not directly on-chain (whitelist is additive)
// Bot-side filtering:
async fn filter_active_pools(all_pools: &[Address]) -> Vec<Address> {
    let mut active = Vec::new();
    for pool in all_pools {
        if pool_metrics.get(pool).score >= MIN_SCORE {
            active.push(*pool);
        }
    }
    active
}
```

## MONITORING METRICS

```
┌─────────────────────────────────────────────────────────────┐
│                  POOL MONITORING DASHBOARD                  │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  DISCOVERED POOLS:                                          │
│  ├── Total:     2,456                                       │
│  ├── Filtered:  234 (passed initial filter)                 │
│  └── Whitelisted: 47 (score >= 60)                          │
│                                                             │
│  TOP 5 BY SCORE:                                            │
│  ┌───────────────────────────────────────────────────────┐  │
│  │ Pool              │ DEX      │ TVL      │ Score       │  │
│  ├───────────────────┼──────────┼──────────┼─────────────┤  │
│  │ 0x1234...WETH/USDC│ UniV3    │ $15.2M   │ 92          │  │
│  │ 0x5678...WETH/DAI │ Aero     │ $8.7M    │ 87          │  │
│  │ 0x9ABC...cbETH/ETH│ UniV3    │ $5.4M    │ 81          │  │
│  │ 0xDEF0...USDC/DAI │ Sushi    │ $3.2M    │ 78          │  │
│  │ 0x4567...AERO/WETH│ Aero     │ $2.8M    │ 75          │  │
│  └───────────────────────────────────────────────────────┘  │
│                                                             │
│  RECENT DISCOVERIES (24h):                                  │
│  ├── New pools: 12                                          │
│  ├── Added to whitelist: 3                                  │
│  └── Removed (low score): 1                                 │
│                                                             │
│  ARBITRAGE STATS (7d):                                      │
│  ├── Profitable routes: 847                                 │
│  ├── Total profit: 2.34 ETH                                 │
│  └── Avg profit per route: 0.0028 ETH                       │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

## ÇIKIŞ RAPORU

```
═══════════════════════════════════════════════════════════════
🔍 POOL DISCOVERY REPORT
═══════════════════════════════════════════════════════════════
Tarih: [timestamp]
Period: Last 24 hours
Network: Base (Chain ID: 8453)

📊 DISCOVERY SUMMARY:

┌─────────────────────────────────────────────────────────────┐
│ METRIC                         │ VALUE                      │
├────────────────────────────────┼────────────────────────────┤
│ New pools discovered           │ 47                         │
│ Passed initial filter          │ 12                         │
│ High score (>= 70)             │ 5                          │
│ Added to whitelist             │ 3                          │
│ Existing pools updated         │ 234                        │
│ Pools removed (low score)      │ 1                          │
└─────────────────────────────────────────────────────────────┘

📈 NEW HIGH-POTENTIAL POOLS:

1. 0x1234...5678 (UniV3)
   ├── Pair: WETH/USDC
   ├── Fee: 0.05%
   ├── TVL: $2.4M
   ├── 24h Volume: $890K
   └── Score: 78 ✅ ADDED

2. 0xABCD...EF01 (Aerodrome)
   ├── Pair: AERO/WETH
   ├── Fee: 0.30%
   ├── TVL: $1.1M
   ├── 24h Volume: $420K
   └── Score: 72 ✅ ADDED

3. 0x9876...5432 (SushiV3)
   ├── Pair: cbETH/WETH
   ├── Fee: 0.01%
   ├── TVL: $3.2M
   ├── 24h Volume: $1.2M
   └── Score: 85 ✅ ADDED

⚠️ WATCHLIST (Score 50-70):
├── 0xAAAA... (DEGEN/WETH) - Score: 62
├── 0xBBBB... (BRETT/USDC) - Score: 58
└── 0xCCCC... (TOSHI/WETH) - Score: 55

🔴 REMOVED (Score < 40):
└── 0xDDDD... (MEME/WETH) - Score dropped to 32

═══════════════════════════════════════════════════════════════
```

## RISK CONSIDERATIONS

```
⚠️ Pool Risk Factors:
├── Low TVL (< $50k): High slippage, manipulation risk
├── New token: Rug pull potential
├── Single LP: Withdrawal = pool death
├── Unusual fee tier: Limited counterparties
└── Low volume: Arbitrage opportunities rare

✅ Safe Pool Indicators:
├── High TVL (> $1M)
├── Established token pairs (WETH, USDC, DAI)
├── Multiple DEX presence (counterparties)
├── Consistent volume
└── Verified contracts
```

---

*"Keşif, kârlılığın başlangıcıdır."*
