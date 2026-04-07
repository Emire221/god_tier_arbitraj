---
name: "Full System Test"
description: "Bot + Contract entegrasyon testi — End-to-end arbitraj validasyonu"
---

# 🧪 FULL SYSTEM TEST SKILL

> **Amaç:** Bot/ ve Contract/ arasında tam entegrasyon testi — REVM simülasyonu, Foundry testi ve on-chain validasyon.

## KULLANIM SENARYOLARI

### Senaryo 1: Tam Sistem Testi
```
"Tüm sistemi test et"
"End-to-end arbitraj testini çalıştır"
```

### Senaryo 2: Regression Test
```
"Son değişiklikler regresyona yol açtı mı?"
"Performans baseline'ı kontrol et"
```

### Senaryo 3: Pre-Deploy Validasyon
```
"Production'a çıkmadan önce tam validasyon yap"
"Release candidate'i doğrula"
```

## OTONOM İŞ AKIŞI

```
┌─────────────────────────────────────────────────────────────┐
│                    FULL SYSTEM TEST PIPELINE                │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  STAGE 1: RUST BOT TESTS                                    │
│  ├── cargo check                                            │
│  ├── cargo clippy -- -D warnings                            │
│  ├── cargo test --release                                   │
│  └── cargo test --release -- --ignored proptest             │
│                                                             │
│  STAGE 2: SOLIDITY CONTRACT TESTS                           │
│  ├── forge build                                            │
│  ├── forge test -vvv                                        │
│  ├── forge test --gas-report                                │
│  └── forge test --fork-url $BASE_RPC_URL                    │
│                                                             │
│  STAGE 3: REVM-FOUNDRY COMPARISON                           │
│  ├── REVM simulation (Bot)                                  │
│  ├── Foundry fork test (Contract)                           │
│  └── Wei-level profit comparison                            │
│                                                             │
│  STAGE 4: CALLDATA ENCODING TEST                            │
│  ├── Bot calldata generation                                │
│  ├── Contract calldata parsing                              │
│  └── Byte-perfect match verification                        │
│                                                             │
│  STAGE 5: INTEGRATION TEST                                  │
│  ├── Shadow mode simulation                                 │
│  ├── Gas estimation validation                              │
│  └── Latency benchmark                                      │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

## TEST STAGES

### Stage 1: Rust Bot Tests

```bash
cd Bot

# 1. Syntax check
cargo check
# Expected: Compiling arbitraj_botu v25.0.0

# 2. Lint check
cargo clippy -- -D warnings
# Expected: 0 warnings

# 3. Unit tests
cargo test --release
# Expected: test result: ok. X passed; 0 failed

# 4. Property tests
cargo test --release -- --ignored proptest
# Expected: proptest result: ok
```

**Başarı Kriterleri:**
- ✅ Compile success
- ✅ 0 clippy warnings
- ✅ All tests pass
- ✅ Proptest invariants hold

### Stage 2: Solidity Contract Tests

```bash
cd Contract

# 1. Build
forge build
# Expected: Compiler run successful

# 2. Tests
forge test -vvv
# Expected: Test result: ok. X passed

# 3. Gas report
forge test --gas-report
# Expected: Gas report with acceptable limits

# 4. Fork tests
forge test --fork-url $BASE_RPC_URL -vvv
# Expected: Fork tests pass on real state
```

**Başarı Kriterleri:**
- ✅ Build success
- ✅ All tests pass
- ✅ Gas within limits
- ✅ Fork tests pass

### Stage 3: REVM-Foundry Comparison

```
REVM Simülasyon → profit_wei, gas_used, amount_out
Foundry Test    → profit_wei, gas_used, amount_out

Karşılaştırma Toleransları:
├── Profit: ±0.01% (1 bps)
├── Gas: ±10%
└── Amount: ±1 wei
```

**Test Adımları:**
1. Aynı pool pair için REVM simülasyonu çalıştır
2. Aynı parametrelerle Foundry fork testi çalıştır
3. Sonuçları wei bazında karşılaştır

### Stage 4: Calldata Encoding Test

```
Bot (Rust) → encode_calldata() → bytes
Contract (Solidity) → decode in fallback() → parsed values

Doğrulama:
├── Pool addresses match
├── Amount matches
├── Direction matches
├── minProfit matches
└── Deadline matches
```

**134-byte Format Test:**
```
[0x00..0x14]  Pool A     ✓
[0x14..0x28]  Pool B     ✓
[0x28..0x3C]  owedToken  ✓
[0x3C..0x50]  recvToken  ✓
[0x50..0x70]  amount     ✓
[0x70]        uniDir     ✓
[0x71]        aeroDir    ✓
[0x72..0x82]  minProfit  ✓
[0x82..0x86]  deadline   ✓
```

### Stage 5: Integration Test

```bash
# Shadow mode simulation
RUST_LOG=info cargo run --release -- --shadow-mode

# Metrics to capture:
├── Pool sync latency: < 5ms
├── Simulation time: < 50ms
├── TX encoding: < 1ms
└── Total cycle: < 100ms
```

## PERFORMANS BASELINEs

| Metrik | Hedef | Kabul Edilebilir | Kritik |
|--------|-------|------------------|--------|
| Pool sync | < 5ms | < 10ms | > 20ms |
| REVM sim | < 50ms | < 100ms | > 200ms |
| Calldata encode | < 1ms | < 5ms | > 10ms |
| Total cycle | < 100ms | < 200ms | > 500ms |
| Contract gas | < 200k | < 300k | > 500k |

## ÇIKIŞ RAPORU

```
═══════════════════════════════════════════════════════════════
🧪 FULL SYSTEM TEST REPORT
═══════════════════════════════════════════════════════════════
Zaman: [timestamp]
Genel Durum: ✅ PASS | ⚠️ WARN | ❌ FAIL

📊 STAGE SONUÇLARI:

┌─────────────────────────────────────────────────────────────┐
│ STAGE 1: RUST BOT TESTS                                     │
├─────────────────────────────────────────────────────────────┤
│ cargo check:       ✅ PASS (2.3s)                           │
│ cargo clippy:      ✅ 0 warnings                            │
│ cargo test:        ✅ 47/47 passed (8.5s)                   │
│ proptest:          ✅ 1000 cases (12.3s)                    │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│ STAGE 2: SOLIDITY CONTRACT TESTS                            │
├─────────────────────────────────────────────────────────────┤
│ forge build:       ✅ PASS (4.2s)                           │
│ forge test:        ✅ 12/12 passed (15.6s)                  │
│ gas report:        ✅ within limits                         │
│ fork test:         ✅ PASS (32.1s)                          │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│ STAGE 3: REVM-FOUNDRY COMPARISON                            │
├─────────────────────────────────────────────────────────────┤
│ Profit match:      ✅ 0.003% deviation                      │
│ Gas match:         ✅ 2.1% deviation                        │
│ Amount match:      ✅ 0 wei difference                      │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│ STAGE 4: CALLDATA ENCODING                                  │
├─────────────────────────────────────────────────────────────┤
│ 134-byte format:   ✅ Byte-perfect match                    │
│ Multi-hop format:  ✅ Byte-perfect match                    │
│ Edge cases:        ✅ All handled                           │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│ STAGE 5: INTEGRATION                                        │
├─────────────────────────────────────────────────────────────┤
│ Pool sync:         ✅ 3.2ms (target: <5ms)                  │
│ REVM simulation:   ✅ 42ms (target: <50ms)                  │
│ Total cycle:       ✅ 78ms (target: <100ms)                 │
└─────────────────────────────────────────────────────────────┘

⏱️ TOPLAM SÜRE: 74.0s

📈 ÖNCEKİ SONUÇLARLA KARŞILAŞTIRMA:
├── Test count:     47 → 47 (=)
├── Pass rate:      100% → 100% (=)
├── Avg latency:    82ms → 78ms (-4.9%)
└── Gas usage:      198k → 195k (-1.5%)

🎯 SONUÇ: Sistem production'a hazır.
═══════════════════════════════════════════════════════════════
```

## HIZLI KOMUTLAR

```bash
# One-liner: Full system test
cd Bot && cargo test --release && cd ../Contract && forge test -vvv

# With gas report
cd Bot && cargo test --release && cd ../Contract && forge test --gas-report

# Parallel (if available)
(cd Bot && cargo test --release) & (cd Contract && forge test) & wait
```

## HATA DURUMLARINDA

### Rust Test Failure
```
→ @rust-ninja'ya yönlendir
→ Hata mesajını analiz et
→ Düzeltme sonrası tekrar test
```

### Solidity Test Failure
```
→ @solidity-pro'ya yönlendir
→ Revert reason analizi
→ Gas report kontrolü
```

### Comparison Mismatch
```
→ REVM-Foundry skill ile detaylı analiz
→ Tick bitmap / liquidity state kontrolü
→ Fee hesaplama doğrulama
```

---

*"Sistem bütünlüğü, parçaların toplamından büyüktür."*
