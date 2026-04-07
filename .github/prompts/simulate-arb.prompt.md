---
agent: agent
description: "Arbitraj fırsatını REVM ile simüle eder ve kârlılık analizi yapar"
---

# 🎯 Arbitraj Simülasyonu

## Görev
Verilen pool adresleri ve parametrelerle **arbitraj simülasyonu** çalıştır ve kârlılık analizi yap.

## Gerekli Girdiler

### Pool Bilgileri
```
Pool A: <adres>
Pool B: <adres>
Token: <token_adresi>
Input Amount: <miktar>
```

### Simülasyon Parametreleri
- Gas price (gwei)
- Block number (opsiyonel - current veya historical)
- Min profit threshold

## Simülasyon Adımları

### 1. Pool State Çekme
```rust
// Bot/src/state_sync.rs referans
let pool_a_state = fetch_pool_state(pool_a_address).await?;
let pool_b_state = fetch_pool_state(pool_b_address).await?;
```

### 2. REVM Simülasyonu
```rust
// Bot/src/simulator.rs referans
let sim_result = simulate_arbitrage(
    pool_a_state,
    pool_b_state,
    input_amount,
    &revm_db,
)?;
```

### 3. Kârlılık Hesaplama
```
Gross Profit = Output - Input
Gas Cost = Gas Used × Gas Price
Net Profit = Gross Profit - Gas Cost
ROI = (Net Profit / Input) × 100%
```

## Çıktı Formatı

### Simülasyon Sonuçları
```
╔══════════════════════════════════════════════════╗
║           ARBITRAJ SİMÜLASYON RAPORU             ║
╠══════════════════════════════════════════════════╣
║ Pool A → Pool B Route                            ║
╠══════════════════════════════════════════════════╣
║ Input:        1.000000 ETH                       ║
║ Output:       1.005234 ETH                       ║
║ Gross Profit: 0.005234 ETH ($15.70)              ║
║ Gas Used:     245,000                            ║
║ Gas Cost:     0.000735 ETH ($2.21)               ║
║ Net Profit:   0.004499 ETH ($13.49)              ║
║ ROI:          0.45%                              ║
╠══════════════════════════════════════════════════╣
║ Status: ✅ PROFITABLE                            ║
╚══════════════════════════════════════════════════╝
```

### Detaylı Analiz
1. **Swap Path**: Token flow detayı
2. **Price Impact**: Her pool'daki fiyat etkisi
3. **Tick Transitions**: Uniswap V3 tick geçişleri
4. **Gas Breakdown**: Fonksiyon bazlı gas kullanımı

## Doğrulama Kontrolleri

### REVM vs On-chain Tutarlılık
```rust
// Wei-level precision kontrolü
assert_eq!(
    sim_output.amount_out,
    expected_onchain_output,
    "Simulation mismatch!"
);
```

### Staleness Check
```rust
// Pool state 2 bloktan eski mi?
if pool_state.block_number < current_block - 2 {
    warn!("Stale pool data, re-fetch required");
}
```

## Komutlar

### Rust Bot'tan Simülasyon
```bash
cd Bot
cargo run --release -- simulate \
  --pool-a 0x... \
  --pool-b 0x... \
  --amount 1.0 \
  --gas-price 0.001
```

### Foundry ile Test
```bash
cd Contract
forge test --match-test test_SimulatedArbitrage -vvvv
```

## Risk Uyarıları
- ⚠️ Simülasyon ≠ Gerçek sonuç (MEV, slippage)
- ⚠️ Gas price volatility
- ⚠️ Pool state değişkenliği
- ⚠️ Private RPC kullan (frontrun koruması)
