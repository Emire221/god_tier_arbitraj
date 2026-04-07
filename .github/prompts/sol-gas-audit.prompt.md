---
agent: agent
description: "Solidity kontratını gas optimizasyonu açısından denetler - EIP-1153, assembly, custom errors"
---

# ⛽ Solidity Gas Denetimi

## Görev
Seçili Solidity kodunu **Gas Golfing** tekniklerine göre optimize et.

## Gas Optimizasyon Kontrolleri

### 🔴 KRİTİK (Yüksek Tasarruf)

#### 1. Storage vs Transient Storage
| Opcode | Gas (Cold) | Gas (Warm) |
|--------|------------|------------|
| SSTORE | 20,000 | 5,000 |
| SLOAD | 2,100 | 100 |
| TSTORE | ~100 | ~100 |
| TLOAD | ~100 | ~100 |

```solidity
// ❌ PAHALI: Kalıcı storage
uint256 public reentrancyLock;
reentrancyLock = 1;

// ✅ UCUZ: EIP-1153 Transient Storage
assembly {
    tstore(0xFF, 1)  // Lock
    // ... işlem ...
    tstore(0xFF, 0)  // Unlock
}
```

#### 2. Custom Errors vs String Revert
```solidity
// ❌ PAHALI: ~5000+ gas
require(msg.sender == owner, "Not authorized");
revert("Insufficient balance");

// ✅ UCUZ: ~100 gas (4-byte selector)
error Unauthorized();
error InsufficientBalance();
if (msg.sender != owner) revert Unauthorized();
```

#### 3. Immutable Variables
```solidity
// ❌ PAHALI: Storage read her çağrıda
address public owner;

// ✅ UCUZ: Bytecode'a gömülü
address public immutable owner;
```

### 🟡 ORTA (Orta Tasarruf)

#### 4. Calldata vs Memory
```solidity
// ❌ Gereksiz kopyalama
function process(bytes memory data) external { }

// ✅ Doğrudan calldata okuma
function process(bytes calldata data) external { }
```

#### 5. Assembly Optimizasyonu
```solidity
// ❌ ABI decode overhead
(address poolA, uint256 amount) = abi.decode(msg.data, (address, uint256));

// ✅ Doğrudan calldata okuma
assembly {
    poolA := shr(96, calldataload(0x04))
    amount := calldataload(0x24)
}
```

### 🟢 DÜŞÜK (Küçük Tasarruf)

#### 6. Unchecked Arithmetic
```solidity
// Overflow mümkün değilse
unchecked {
    i++;  // ~3 gas tasarruf per iteration
}
```

#### 7. Tight Variable Packing
```solidity
// ❌ 2 slot
uint256 a;
uint128 b;
uint128 c;

// ✅ 2 slot (packed)
uint128 b;
uint128 c;
uint256 a;
```

## Çıktı Formatı

### Gas Raporu Tablosu
| Satır | Mevcut Gas | Önerilen Gas | Tasarruf | Değişiklik |
|-------|------------|--------------|----------|------------|
| ? | ? | ? | ?% | Açıklama |

### Önerilen Kod Değişiklikleri
Her optimizasyon için before/after kod örneği.

## Doğrulama
```bash
forge test --gas-report
forge snapshot --diff
```

## Proje Bağlamı
- Solidity: ^0.8.27
- EVM Target: Cancun (EIP-1153 destekli)
- Optimizer: via_ir=true, runs=1_000_000
