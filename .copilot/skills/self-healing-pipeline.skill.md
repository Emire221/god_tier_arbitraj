---
name: "Self-Healing Pipeline"
description: "Otonom hata tespiti, kök neden analizi ve self-healing döngüsü"
---

# 🔄 SELF-HEALING PIPELINE SKILL

> **Amaç:** Hataları otomatik tespit et, kök nedeni analiz et ve ilgili ajana yönlendirerek düzelt — maksimum 5 iterasyon.

## KULLANIM SENARYOLARI

### Senaryo 1: Build/Test Hatası
```
"Bu hatayı düzelt: [cargo/forge output]"
"Test başarısız, kök nedeni bul"
```

### Senaryo 2: Runtime Error
```
"Panic yakalandı, düzelt"
"Revert reason: InsufficientProfit()"
```

### Senaryo 3: Latency Regression
```
"Simulation 150ms'ye çıktı, optimize et"
"Pool sync yavaşladı"
```

## SELF-HEALING DÖNGÜSÜ

```
┌─────────────────────────────────────────────────────────────┐
│                   SELF-HEALING LOOP                         │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│   ┌──────────────┐                                          │
│   │ 1. DETECT    │ ◄─── Terminal output / Log / Metric      │
│   └──────┬───────┘                                          │
│          │                                                  │
│          ▼                                                  │
│   ┌──────────────┐                                          │
│   │ 2. CLASSIFY  │ ◄─── 🔴 Critical / 🟠 High / 🟡 Medium   │
│   └──────┬───────┘                                          │
│          │                                                  │
│          ▼                                                  │
│   ┌──────────────┐                                          │
│   │ 3. ROOT CAUSE│ ◄─── Pattern matching + Stack trace      │
│   └──────┬───────┘                                          │
│          │                                                  │
│          ▼                                                  │
│   ┌──────────────┐                                          │
│   │ 4. ASSIGN    │ ◄─── @rust-ninja / @solidity-pro         │
│   └──────┬───────┘                                          │
│          │                                                  │
│          ▼                                                  │
│   ┌──────────────┐                                          │
│   │ 5. FIX       │ ◄─── Ajan düzeltme uygular               │
│   └──────┬───────┘                                          │
│          │                                                  │
│          ▼                                                  │
│   ┌──────────────┐                                          │
│   │ 6. VERIFY    │ ◄─── cargo test / forge test             │
│   └──────┬───────┘                                          │
│          │                                                  │
│      ┌───┴───┐                                              │
│      │       │                                              │
│     ✅      ❌                                               │
│    DONE   RETRY (max 5)                                     │
│             │                                               │
│             └──────► 3. ROOT CAUSE (alternatif yaklaşım)    │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

## HATA PATTERN KATALOGU

### 🔴 KRİTİK (Anında Müdahale)

| Pattern | Örnek | Ajan | Düzeltme |
|---------|-------|------|----------|
| Rust panic | `thread 'main' panicked` | @rust-ninja | unwrap → ? |
| Solidity revert | `execution reverted` | @solidity-pro | Condition fix |
| Reentrancy | `Locked()` | @solidity-pro | Guard check |
| Out of gas | `OutOfGas` | @solidity-pro | Gas optimize |
| Memory overflow | `out of memory` | @rust-ninja | Allocation fix |

### 🟠 YÜKSEK (Sonraki İterasyon)

| Pattern | Örnek | Ajan | Düzeltme |
|---------|-------|------|----------|
| Compile error | `error[E` | @rust-ninja | Syntax/type fix |
| Forge error | `Error:`, `TypeError:` | @solidity-pro | Contract fix |
| Test failure | `FAILED`, `[FAIL]` | İlgili ajan | Test/impl fix |
| Type mismatch | `mismatched types` | @rust-ninja | Type annotation |

### 🟡 ORTA (Backlog)

| Pattern | Örnek | Ajan | Düzeltme |
|---------|-------|------|----------|
| Warning | `warning:` | İlgili ajan | Cleanup |
| Clippy | `clippy::` | @rust-ninja | Best practice |
| Latency spike | `> 100ms` | @rust-ninja | Optimization |
| Gas spike | `> 300k` | @solidity-pro | Gas golf |

## İTERASYON STRATEJİLERİ

### İterasyon 1: Standart Düzeltme
```
1. Hata mesajını analiz et
2. Doğrudan düzeltme uygula
3. Test et
```

### İterasyon 2: Alternatif Yaklaşım
```
1. Farklı bir çözüm dene
2. API/library değişikliği düşün
3. Test et
```

### İterasyon 3: Kök Neden Derinleştirme
```
1. Stack trace'i detaylı incele
2. Bağımlılık zincirini kontrol et
3. Edge case analizi yap
```

### İterasyon 4: Minimal Repro
```
1. Hatayı izole eden minimal kod oluştur
2. Bağımlılıkları kaldır
3. Temel nedeni ortaya çıkar
```

### İterasyon 5: Human Escalation
```
1. Tüm girişimleri dokümante et
2. Bulguları özetle
3. İnsan müdahalesi iste
```

## RUST HATA DÜZELTMELERİ

### unwrap() Panic
```rust
// ÖNCE (hatalı)
let value = result.unwrap();

// SONRA (düzeltilmiş)
let value = result.map_err(|e| eyre::eyre!("Context: {}", e))?;
// veya
let value = result.unwrap_or_default();
```

### Ownership Error
```rust
// ÖNCE: borrow of moved value
let state = shared_state;
process(state);
use_again(state); // ERROR

// SONRA: Arc clone
let state = Arc::clone(&shared_state);
process(Arc::clone(&state));
use_again(state);
```

### Send Trait Bound
```rust
// ÖNCE: trait bound `T: Send` not satisfied
let handle = tokio::spawn(async move { use_non_send(value) });

// SONRA: spawn_local veya Arc<Mutex<T>>
let value = Arc::new(parking_lot::Mutex::new(value));
let handle = tokio::spawn(async move {
    let guard = value.lock();
    // ...
});
```

## SOLIDITY HATA DÜZELTMELERİ

### Stack Too Deep
```solidity
// ÖNCE: Stack too deep
function complex(a,b,c,d,e,f,g,h,i,j,k,l,m,n,o,p,q) {
    // 17 parameters
}

// SONRA: Struct kullan
struct Params { a; b; c; ... }
function complex(Params calldata params) {
    // Single parameter
}
```

### Gas Optimization
```solidity
// ÖNCE: High gas (string revert)
require(condition, "Long error message");

// SONRA: Custom error
error ConditionFailed();
if (!condition) revert ConditionFailed();
```

### Reentrancy Fix
```solidity
// ÖNCE: Vulnerable
function withdraw() external {
    token.transfer(msg.sender, amount); // external call
    balance[msg.sender] = 0; // state change after
}

// SONRA: CEI pattern + transient lock
function withdraw() external {
    uint256 locked;
    assembly { locked := tload(0xFF) }
    if (locked != 0) revert Locked();
    assembly { tstore(0xFF, 1) }

    balance[msg.sender] = 0; // state change first
    token.transfer(msg.sender, amount);

    assembly { tstore(0xFF, 0) }
}
```

## ESCALATION KRİTERLERİ

```
Human Escalation Gerekli:
├── 5 iterasyon sonunda çözüm yok
├── Güvenlik açığı tespit edildi
├── Anayasa ihlali gerekiyor
├── Birden fazla ajan koordinasyonu gerekli
└── Dış bağımlılık problemi (RPC, network, etc.)
```

## ÇIKIŞ RAPORU

```
═══════════════════════════════════════════════════════════════
🔄 SELF-HEALING REPORT
═══════════════════════════════════════════════════════════════
Hata: [kısa açıklama]
Seviye: 🔴 KRİTİK | 🟠 YÜKSEK | 🟡 ORTA
Durum: ✅ ÇÖZÜLDÜ | ⚠️ ESCALATİON | ❌ BAŞARISIZ

📋 İTERASYON GEÇMİŞİ:

İterasyon 1/5:
├── Yaklaşım: [standart düzeltme]
├── Sonuç: ❌ Başarısız
└── Öğrenim: [ne öğrendik]

İterasyon 2/5:
├── Yaklaşım: [alternatif]
├── Sonuç: ✅ Başarılı
└── Düzeltme: [uygulanan fix]

🔧 UYGULANAN DÜZELTME:
Dosya: [path:line]
Ajan: @[rust-ninja|solidity-pro]
```diff
- let value = result.unwrap();
+ let value = result?;
```

✅ DOĞRULAMA:
├── cargo check:  ✅
├── cargo clippy: ✅
├── cargo test:   ✅ (47/47)
└── forge test:   ✅ (12/12)

⏱️ Toplam Süre: X.Xs
İterasyon Sayısı: Y/5
═══════════════════════════════════════════════════════════════
```

## OTOMASYON HOOK'LARI

### Pre-Commit Hook
```bash
#!/bin/bash
# .git/hooks/pre-commit

# Rust checks
cd Bot
cargo check || exit 1
cargo clippy -- -D warnings || exit 1

# Solidity checks
cd ../Contract
forge build || exit 1
```

### CI Integration
```yaml
# .github/workflows/self-heal.yml
on: [push, pull_request]

jobs:
  self-heal:
    steps:
      - name: Detect Issues
        run: |
          cargo check 2>&1 | tee rust_output.txt
          forge build 2>&1 | tee sol_output.txt

      - name: Self-Heal (if needed)
        if: failure()
        run: |
          # Trigger self-healing skill
```

---

*"Her hata, sistemin güçlenmesi için bir fırsattır."*
