---
name: "Autonomous Solidity Contract"
description: "Tam otonom Contract/ geliştirme, test ve deploy becerisi — Gas golfing, güvenlik, Foundry"
---

# ⛽ AUTONOMOUS SOLIDITY CONTRACT SKILL

> **Amaç:** Contract/ dizininde tam otonom geliştirme — gas optimizasyonu, güvenlik güçlendirme, test yazımı ve deployment.

## KULLANIM SENARYOLARI

### Senaryo 1: Yeni Özellik Ekleme
```
"Aave V3 flashloan entegrasyonu ekle"
"Yeni callback tipi ekle"
```

### Senaryo 2: Gas Optimizasyonu
```
"Multi-hop fonksiyonunda gas'ı %20 düşür"
"Whitelist kontrolünü optimize et"
```

### Senaryo 3: Güvenlik Güçlendirme
```
"Yeni saldırı vektörüne karşı koruma ekle"
"Callback doğrulamasını güçlendir"
```

### Senaryo 4: Full Deploy Pipeline
```
"Kontratı Base mainnet'e deploy et"
"Upgrade hazırlığı yap"
```

## OTONOM İŞ AKIŞI

```
1. DISCOVERY (Otomatik)
   ├── Mevcut kontrat analizi (forge build)
   ├── Test durumu (forge test)
   ├── Gas raporu (forge test --gas-report)
   └── Güvenlik taraması

2. PLANNING
   ├── Değişiklik kapsamı
   ├── Gas impact tahmini
   └── Güvenlik risk değerlendirmesi

3. IMPLEMENTATION
   ├── Custom errors (string revert yasak)
   ├── Immutable variables (mümkün olan yerde)
   ├── Assembly calldata okuma
   ├── EIP-1153 transient storage
   └── Unchecked arithmetic (güvenli yerlerde)

4. VALIDATION
   ├── forge build
   ├── forge test -vvv
   ├── forge test --gas-report
   ├── Fuzz testing (--fuzz-runs 10000)
   └── Fork testing

5. SECURITY AUDIT
   ├── Reentrancy check
   ├── Callback validation
   ├── Access control
   ├── Overflow/underflow
   └── MEV vulnerability scan

6. DEPLOY
   ├── Dry run (anvil fork)
   ├── Gas estimation
   └── Mainnet deploy
```

## PROJE BAĞLAMI (Sabit Değerler)

| Parametre | Değer |
|-----------|-------|
| Solidity | 0.8.27 |
| EVM | cancun |
| Optimizer | true |
| Optimizer runs | 1,000,000 |
| via_ir | true |
| Network | Base L2 |

## KONTRAT MİMARİSİ

```
Contract/src/Arbitraj.sol
├── IMMUTABLES
│   ├── executor (address) — Sıcak cüzdan
│   └── admin (address) — Soğuk cüzdan
├── STORAGE
│   └── poolWhitelist (mapping)
├── TRANSIENT STORAGE
│   ├── 0x00: expectedPool
│   ├── 0x01: aeroPool
│   ├── 0x02: aeroDirection
│   ├── 0x03: owedToken
│   ├── 0x04: receivedToken
│   ├── 0x10-0x1F: multi-hop data
│   └── 0xFF: reentrancy lock
├── ENTRY POINTS
│   ├── fallback() — 134B calldata
│   ├── withdrawToken()
│   ├── withdrawETH()
│   └── executorBatchAddPools()
└── CALLBACKS
    ├── uniswapV3SwapCallback()
    └── pancakeV3SwapCallback()
```

## GAS GOLFING ŞABLONLARI

### Custom Error Pattern

```solidity
// ✅ DOĞRU: 4-byte selector
error Unauthorized();
error InvalidCaller();
error InsufficientProfit();
error PoolNotWhitelisted();
error DeadlineExpired();
error Locked();
error ZeroAmount();
error ZeroAddress();

// Kullanım
if (msg.sender != executor) revert Unauthorized();

// ❌ YASAK
require(msg.sender == executor, "Not authorized");
```

### Assembly Calldata Reading

```solidity
// ✅ DOĞRU: ABI decode bypass
assembly {
    // 20-byte address: shr(96, ...)
    poolA := shr(96, calldataload(0x00))

    // 32-byte uint256: doğrudan
    amount := calldataload(0x50)

    // 1-byte uint8: shr(248, ...)
    direction := shr(248, calldataload(0x70))

    // 16-byte uint128: shr(128, ...)
    minProfit := shr(128, calldataload(0x72))

    // 4-byte uint32: shr(224, ...)
    deadline := shr(224, calldataload(0x82))
}
```

### Transient Storage

```solidity
// ✅ DOĞRU: EIP-1153 (~100 gas)
assembly {
    tstore(0xFF, 1)        // Reentrancy lock SET
    tstore(0x00, poolAddr) // Context write
}

// Okuma
assembly {
    let locked := tload(0xFF)
    let expected := tload(0x00)
}

// Temizlik
assembly {
    tstore(0xFF, 0)
}

// ❌ YASAK: Kalıcı SSTORE (20000/5000 gas)
```

### Unchecked Arithmetic

```solidity
// ✅ DOĞRU: Overflow impossible (güvenli)
if (balAfter <= balBefore) revert NoProfitRealized();
uint256 profit;
unchecked {
    profit = balAfter - balBefore;
}
```

## GÜVENLİK ŞABLONLARI

### Reentrancy Guard

```solidity
// Her external call öncesi
uint256 locked;
assembly { locked := tload(0xFF) }
if (locked != 0) revert Locked();
assembly { tstore(0xFF, 1) }

// ... external calls ...

// İşlem sonunda
assembly { tstore(0xFF, 0) }
```

### Callback Validation

```solidity
function uniswapV3SwapCallback(
    int256 amount0Delta,
    int256 amount1Delta,
    bytes calldata
) external {
    address expected;
    assembly { expected := tload(0x00) }
    if (msg.sender != expected) revert InvalidCaller();
    // ...
}
```

### Access Control

```solidity
// Executor-only (arbitraj)
fallback() external {
    if (msg.sender != executor) revert Unauthorized();
}

// Admin-only (fon yönetimi)
function withdrawToken(address token, uint256 amount) external {
    if (msg.sender != admin) revert Unauthorized();
}
```

## TEST PROTOKOLÜ

### Otomatik Test Sırası

```bash
# 1. Build (~5s)
forge build

# 2. All tests (~30s)
forge test -vvv

# 3. Gas report (~30s)
forge test --gas-report

# 4. Fuzz testing (~60s)
forge test --fuzz-runs 10000

# 5. Fork testing (~60s)
forge test --fork-url $BASE_RPC_URL -vvv
```

### Test Kategorileri

| Kategori | Prefix | Açıklama |
|----------|--------|----------|
| Unit | `test_` | Tek fonksiyon testi |
| Revert | `test_RevertWhen_` | Hata koşulları |
| Fuzz | `testFuzz_` | Random input |
| Fork | `test_Fork_` | Mainnet state |
| Gas | `test_Gas_` | Gas limitleri |
| Invariant | `invariant_` | Sistem değişmezleri |

## CALLDATA FORMAT

### 2-Pool (134 byte)
```
Offset   Size   Field
────────────────────────
0x00     20B    Pool A
0x14     20B    Pool B
0x28     20B    owedToken
0x3C     20B    receivedToken
0x50     32B    amount
0x70      1B    uniDirection
0x71      1B    aeroDirection
0x72     16B    minProfit
0x82      4B    deadline
────────────────────────
TOTAL   134B
```

### Multi-Hop (53 + N×21 byte)
```
Offset   Size   Field
────────────────────────
0x00      1B    hopCount
0x01     32B    amount
0x21     16B    minProfit
0x31      4B    deadline
0x35     21B    hop[0]: pool(20) + dir(1)
...
────────────────────────
TOTAL   53 + N×21B
```

## ÇIKIŞ RAPORU

```
═══════════════════════════════════════════════════════════════
⛽ AUTONOMOUS CONTRACT DEVELOPMENT REPORT
═══════════════════════════════════════════════════════════════
Görev: [task description]
Durum: ✅ BAŞARILI | ❌ BAŞARISIZ
Süre: X.Xs

📁 DEĞİŞİKLİKLER:
├── Contract/src/Arbitraj.sol — [change summary]
├── Contract/test/*.sol — [test additions]
└── Contract/foundry.toml — [if changed]

🧪 TEST SONUÇLARI:
├── forge build:     ✅
├── forge test:      ✅ (X/X passed)
├── fuzz tests:      ✅ (10000 runs)
├── fork tests:      ✅
└── coverage:        X%

⛽ GAS PERFORMANS:
├── Function         Before    After     Delta
├── fallback()       45,234    42,100    -3,134 (-6.9%)
├── callback()       12,500    11,800    -700 (-5.6%)
└── withdraw()       28,000    28,000    0

🛡️ GÜVENLİK KONTROL:
├── Reentrancy:      ✅ Protected
├── Callback auth:   ✅ Validated
├── Access control:  ✅ Enforced
├── Overflow:        ✅ Safe
└── MEV:             ✅ minProfit active
═══════════════════════════════════════════════════════════════
```

## KRİTİK KONTROL LİSTESİ

- [ ] Custom errors kullanılmış (string revert yok)
- [ ] Immutable mümkün olan yerde
- [ ] Assembly calldata okuma güvenli
- [ ] tstore/tload (EIP-1153) tercih edilmiş
- [ ] Reentrancy guard aktif (slot 0xFF)
- [ ] Callback msg.sender doğrulaması var
- [ ] Yetki kontrolleri eksiksiz
- [ ] Pool whitelist kontrolü var
- [ ] Deadline mekanizması aktif
- [ ] Gas raporu kabul edilebilir
- [ ] Tüm testler geçiyor
- [ ] Fork test başarılı

---

*"Her wei önemli, her gas hesapta."*
