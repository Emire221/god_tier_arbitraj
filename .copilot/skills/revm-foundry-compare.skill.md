---
name: "REVM-Foundry Compare"
description: "REVM simülasyon sonuçlarını Foundry loglarıyla karşılaştırma becerisi"
---

# 🔬 REVM-FOUNDRY COMPARE SKILL

> **Amaç:** REVM (Rust) simülasyon sonuçlarını Foundry (Solidity) test loglarıyla wei bazında karşılaştırarak tutarlılığı doğrulamak.

## KULLANIM SENARYOLARI

1. **Profit Hesaplama Doğrulama**
   - REVM'de hesaplanan `expected_profit` vs Foundry'de gerçekleşen `profit`

2. **Gas Estimation Doğrulama**
   - REVM `simulated_gas` vs Foundry `gas used`

3. **Swap Output Doğrulama**
   - REVM `amount_out` vs On-chain swap result

## GİRİŞ FORMATI

```json
{
  "revm_result": {
    "expected_profit_wei": "123456789012345678",
    "simulated_gas": 250000,
    "amount_out_wei": "1000000000000000000",
    "simulation_time_ms": 45
  },
  "foundry_result": {
    "actual_profit_wei": "123456789012345600",
    "gas_used": 248532,
    "amount_out_wei": "999999999999999900",
    "test_name": "testArbitrageProfit"
  }
}
```

## KARŞILAŞTIRMA KURALLARI

### 1. Profit Karşılaştırması

```
Tolerans: ±0.01% (1 bps)

deviation = |revm_profit - foundry_profit| / foundry_profit × 100

✅ PASS: deviation < 0.01%
⚠️ WARN: 0.01% <= deviation < 0.1%
❌ FAIL: deviation >= 0.1%
```

### 2. Gas Karşılaştırması

```
Tolerans: ±10% (simülasyon overhead)

deviation = |revm_gas - foundry_gas| / foundry_gas × 100

✅ PASS: deviation < 10%
⚠️ WARN: 10% <= deviation < 20%
❌ FAIL: deviation >= 20%
```

### 3. Amount Out Karşılaştırması

```
Tolerans: ±1 wei (precision limit)

deviation = |revm_amount - foundry_amount|

✅ PASS: deviation <= 1 wei
⚠️ WARN: 1 < deviation <= 100 wei
❌ FAIL: deviation > 100 wei
```

## ÇIKIŞ FORMATI

```
═══════════════════════════════════════════════════════════════
🔬 REVM-FOUNDRY COMPARISON REPORT
═══════════════════════════════════════════════════════════════
Test: [test_name]
Zaman: [timestamp]

📊 PROFIT KARŞILAŞTIRMA:
├── REVM:    123456789012345678 wei (0.123456789 ETH)
├── Foundry: 123456789012345600 wei (0.123456789 ETH)
├── Fark:    78 wei
├── Sapma:   0.000063%
└── Durum:   ✅ PASS

⛽ GAS KARŞILAŞTIRMA:
├── REVM:    250,000
├── Foundry: 248,532
├── Fark:    1,468
├── Sapma:   0.59%
└── Durum:   ✅ PASS

💱 AMOUNT OUT KARŞILAŞTIRMA:
├── REVM:    1000000000000000000 wei (1.0 ETH)
├── Foundry: 999999999999999900 wei (~1.0 ETH)
├── Fark:    100 wei
├── Sapma:   0.00000001%
└── Durum:   ⚠️ WARN (precision loss)

📈 ÖZET:
├── Profit:    ✅
├── Gas:       ✅
├── Amount:    ⚠️
└── Genel:     ✅ PASS (uyarı ile)

⏱️ Performans:
├── REVM Simulation: 45ms
├── Foundry Test:    ~2000ms (estimate)
└── Hız Farkı:       ~44x faster
═══════════════════════════════════════════════════════════════
```

## UYGULAMA ADIMLARI

### Adım 1: REVM Simülasyonunu Çalıştır

```rust
// Bot/src/simulator.rs
let result = simulator.simulate_arbitrage(
    pool_a,
    pool_b,
    amount,
    direction,
)?;

println!("[REVM] profit: {} wei, gas: {}",
    result.profit_wei, result.gas_used);
```

### Adım 2: Foundry Testini Çalıştır

```bash
# Contract/ dizininde
forge test --match-test testArbitrageProfit -vvvv 2>&1 | tee test_output.log
```

### Adım 3: Log Parse Et

```bash
# Profit extraction
grep "profit:" test_output.log | awk '{print $2}'

# Gas extraction
grep "Gas used:" test_output.log | awk '{print $3}'
```

### Adım 4: Karşılaştırma Yap

```python
# Python helper (optional)
def compare_results(revm, foundry, tolerance_bps=1):
    deviation = abs(revm - foundry) / foundry * 10000
    return deviation < tolerance_bps
```

## HATA AYIKLAMA

### Büyük Sapma Durumunda

1. **Tick Bitmap Verisi Güncel mi?**
   - REVM'de kullanılan tick bitmap vs on-chain state

2. **Fee Hesaplaması Tutarlı mı?**
   - Protokol fee, swap fee dahil mi?

3. **Liquidity Değerleri Eşleşiyor mu?**
   - slot0.sqrtPriceX96, liquidity değerleri

4. **Precision Loss Var mı?**
   - f64 → U256 dönüşümlerinde yuvarlama

### Debug Logları

```rust
// Bot/src/simulator.rs — debug mode
#[cfg(debug_assertions)]
{
    eprintln!("[DEBUG] sqrtPriceX96: {}", sqrt_price);
    eprintln!("[DEBUG] liquidity: {}", liquidity);
    eprintln!("[DEBUG] tick: {}", current_tick);
}
```

```solidity
// Contract/test/Arbitraj.t.sol — debug mode
console.log("sqrtPriceX96:", pool.slot0().sqrtPriceX96);
console.log("liquidity:", pool.liquidity());
console.log("tick:", pool.slot0().tick);
```

## ENTEGRASYON

### CI/CD Pipeline

```yaml
# .github/workflows/validate.yml
jobs:
  compare:
    steps:
      - name: Run REVM Simulation
        run: cargo test --release -- simulation_benchmark
        working-directory: Bot

      - name: Run Foundry Tests
        run: forge test --gas-report > foundry_results.txt
        working-directory: Contract

      - name: Compare Results
        run: python scripts/compare_revm_foundry.py
```

### Shadow Mode Entegrasyonu

```rust
// Her simülasyon sonrası otomatik karşılaştırma
if config.shadow_mode() {
    log_comparison_result(
        "shadow_analytics.jsonl",
        revm_result,
        // Foundry sonucu on-chain fork'tan alınır
    );
}
```

## KRİTİK NOTLAR

1. **Wei Bazında Kesinlik:** Arbitraj sisteminde 1 wei bile önemli
2. **Gas Overhead:** REVM simülasyonu Foundry'den ~10% fazla gas gösterebilir
3. **Block State:** Karşılaştırma aynı block state üzerinde yapılmalı
4. **Fork Consistency:** Foundry fork testi REVM ile aynı RPC'yi kullanmalı

---

*"Simülasyon, gerçekliğin aynası olmalı — wei bazında."*
