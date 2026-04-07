---
agent: agent
description: "Başarısız veya beklenmeyen transaction'ları debug eder - revert reason, trace analysis"
---

# 🔧 Transaction Debug

## Görev
Verilen transaction hash veya hata mesajını analiz et ve **kök nedeni** tespit et.

## Gerekli Girdiler

### Seçenek 1: TX Hash
```
TX Hash: 0x...
Network: Base / Ethereum
```

### Seçenek 2: Hata Mesajı
```
Error: execution reverted: ...
veya
Error: OutOfGas
veya
Rust panic/error output
```

## Debug Adımları

### 1. Transaction Trace Analizi
```bash
# Foundry cast ile trace
cast run <tx_hash> --rpc-url $BASE_RPC_URL

# Detaylı trace
cast run <tx_hash> --rpc-url $BASE_RPC_URL --debug
```

### 2. Revert Reason Decode
```bash
# Custom error decode
cast 4byte-decode <selector>

# Veya Foundry ile
cast call --trace <contract> <calldata> --rpc-url $RPC
```

### 3. State Comparison
```bash
# Block N vs Block N-1 state diff
cast storage <contract> <slot> --block <N>
cast storage <contract> <slot> --block <N-1>
```

## Yaygın Hata Kategorileri

### 🚨 Revert Errors

| Error | Olası Neden | Çözüm |
|-------|-------------|-------|
| `Unauthorized()` | msg.sender != executor | Bot wallet kontrolü |
| `InvalidCaller()` | Callback spoofing girişimi | Normal - güvenlik çalışıyor |
| `InsufficientProfit()` | minProfit > actual profit | Threshold ayarla |
| `Locked()` | Reentrancy guard tetiklendi | TX sıralama kontrolü |
| `Expired()` | block.timestamp > deadline | Deadline süresini artır |

### ⛽ Gas Errors

| Error | Olası Neden | Çözüm |
|-------|-------------|-------|
| `OutOfGas` | Gas limit yetersiz | Gas limit artır |
| `EvmError::OutOfGas` | REVM simülasyon hatası | Gas estimate düzelt |

### 🔗 Network Errors

| Error | Olası Neden | Çözüm |
|-------|-------------|-------|
| `nonce too low` | TX çakışması | Nonce yönetimi |
| `replacement underpriced` | Gas price düşük | Gas price artır |
| `intrinsic gas too low` | Calldata büyük | Calldata optimize |

## Rust Bot Debug

### Tracing Logları
```bash
# Verbose logging
RUST_LOG=debug cargo run --release

# Specific module
RUST_LOG=arbitrage_bot::simulator=trace cargo run --release
```

### Error Backtrace
```bash
RUST_BACKTRACE=1 cargo run --release
```

### REVM Debug
```rust
// Simülasyon hata detayı
match sim_result {
    Err(e) => {
        error!("Simulation failed: {:?}", e);
        error!("EVM output: {:?}", e.output());
        error!("Gas used: {}", e.gas_used());
    }
}
```

## Çıktı Formatı

### Debug Raporu
```
╔══════════════════════════════════════════════════╗
║              TRANSACTION DEBUG RAPORU            ║
╠══════════════════════════════════════════════════╣
║ TX Hash: 0xabc...                                ║
║ Status: ❌ REVERTED                              ║
╠══════════════════════════════════════════════════╣
║ Revert Reason: InsufficientProfit()              ║
║ Gas Used: 145,234 / 500,000                      ║
║ Block: 12345678                                  ║
╠══════════════════════════════════════════════════╣
║ KÖK NEDEN:                                       ║
║ Pool state TX gönderilmeden önce değişti.        ║
║ Expected profit: 0.005 ETH                       ║
║ Actual profit: 0.002 ETH                         ║
║ Min threshold: 0.003 ETH                         ║
╠══════════════════════════════════════════════════╣
║ ÖNERİ:                                           ║
║ 1. Pool state freshness kontrolü ekle            ║
║ 2. minProfit threshold'u dinamik yap             ║
║ 3. Private mempool kullan                        ║
╚══════════════════════════════════════════════════╝
```

## İlgili Araçlar
- `cast`: Foundry CLI
- `anvil`: Local fork
- `tenderly`: TX simulation
- `etherscan`: TX explorer
