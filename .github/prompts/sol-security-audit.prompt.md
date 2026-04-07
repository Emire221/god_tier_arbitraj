---
agent: agent
description: "Solidity kontratını güvenlik açıkları açısından denetler - reentrancy, MEV, access control"
---

# 🛡️ Solidity Güvenlik Denetimi

## Görev
Seçili Solidity kodunu **DeFi güvenlik standartlarına** göre denetle.

## Kritik Güvenlik Kontrolleri

### 🚨 SEVİYE 1: KRİTİK (Exploit Riski)

#### 1. Reentrancy
```solidity
// ❌ SAVUNMASIZ
function withdraw(uint256 amount) external {
    (bool success,) = msg.sender.call{value: amount}("");
    balances[msg.sender] -= amount;  // State güncellemesi SONRA
}

// ✅ GÜVENLİ - Checks-Effects-Interactions + Guard
function withdraw(uint256 amount) external {
    // 1. Reentrancy guard (EIP-1153)
    uint256 locked;
    assembly { locked := tload(0xFF) }
    if (locked != 0) revert Locked();
    assembly { tstore(0xFF, 1) }

    // 2. Checks
    if (balances[msg.sender] < amount) revert InsufficientBalance();

    // 3. Effects (state update ÖNCE)
    balances[msg.sender] -= amount;

    // 4. Interactions
    (bool success,) = msg.sender.call{value: amount}("");
    if (!success) revert TransferFailed();

    // 5. Unlock
    assembly { tstore(0xFF, 0) }
}
```

#### 2. Callback Validation
```solidity
// ❌ SAVUNMASIZ - Herkes callback'i çağırabilir
function uniswapV3SwapCallback(int256 amount0, int256 amount1, bytes calldata) external {
    // ... token transfer
}

// ✅ GÜVENLİ - msg.sender doğrulaması
function uniswapV3SwapCallback(int256 amount0, int256 amount1, bytes calldata) external {
    address expected;
    assembly { expected := tload(0x00) }
    if (msg.sender != expected) revert InvalidCaller();
    // ... token transfer
}
```

#### 3. Access Control
```solidity
// Executor ve Admin ayrımı
address public immutable executor;  // Bot - arbitraj çalıştırma
address public immutable admin;     // İnsan - fon yönetimi

fallback() external {
    if (msg.sender != executor) revert Unauthorized();
}

function withdrawToken(address token, uint256 amount) external {
    if (msg.sender != admin) revert Unauthorized();
}
```

### 🟡 SEVİYE 2: YÜKSEK (MEV Riski)

#### 4. Sandwich Protection (minProfit)
```solidity
// ❌ SAVUNMASIZ - Frontrunner fiyatı manipüle edebilir
function executeArbitrage(...) external {
    uint256 profit = swap(...);
    // Profit 0 bile olsa işlem geçer!
}

// ✅ GÜVENLİ - Off-chain hesaplanan minimum kâr
function executeArbitrage(..., uint256 minProfit) external {
    uint256 profit = swap(...);
    if (profit < minProfit) revert InsufficientProfit();
}
```

#### 5. Deadline/Expiry
```solidity
// ❌ SAVUNMASIZ - TX mempool'da bekleyebilir
function swap(uint256 amount) external { }

// ✅ GÜVENLİ - Zaman sınırı
function swap(uint256 amount, uint256 deadline) external {
    if (block.timestamp > deadline) revert Expired();
}
```

### 🟢 SEVİYE 3: ORTA (Operasyonel Risk)

#### 6. Integer Overflow (Solidity 0.8+ otomatik)
- `unchecked` blokları manuel kontrol

#### 7. Return Value Checks
```solidity
// ❌ Return değeri kontrol edilmemiş
token.transfer(to, amount);

// ✅ Kontrol edilmiş
bool success = token.transfer(to, amount);
if (!success) revert TransferFailed();

// ✅ VEYA SafeERC20
token.safeTransfer(to, amount);
```

## Denetim Çıktısı

### Bulgular Tablosu
| ID | Seviye | Kategori | Satır | Açıklama | Öneri |
|----|--------|----------|-------|----------|-------|
| S1 | 🚨 KRİTİK | Reentrancy | ? | ? | ? |

### Risk Değerlendirmesi
- **Kritik**: Doğrudan fon kaybı riski
- **Yüksek**: MEV/manipülasyon riski
- **Orta**: Operasyonel risk
- **Düşük**: Best practice ihlali

## Doğrulama
```bash
# Slither static analysis
slither .

# Foundry tests
forge test -vvv

# Fuzzing
forge test --fuzz-runs 10000
```

## Proje Bağlamı
- Network: Base L2 (OP Stack)
- Flash loan: Uniswap V3 callback
- Executor pattern: EOA bot → Contract
