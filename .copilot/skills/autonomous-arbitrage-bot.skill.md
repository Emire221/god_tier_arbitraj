---
name: "Autonomous Arbitrage Bot"
description: "Tam otonom Bot/ geliştirme, test ve deploy becerisi — Rust, tokio, alloy, revm"
---

# 🤖 AUTONOMOUS ARBITRAGE BOT SKILL

> **Amaç:** Bot/ dizininde tam otonom geliştirme — yeni özellik ekleme, bug düzeltme, performans optimizasyonu ve deployment.

## KULLANIM SENARYOLARI

### Senaryo 1: Yeni Modül Ekleme
```
"Pool discovery için yeni bir DEX entegrasyonu ekle"
"Telegram notification modülünü güncelle"
```

### Senaryo 2: Bug Fix
```
"Bu panic'i düzelt: [hata mesajı]"
"Memory leak tespit edildi, düzelt"
```

### Senaryo 3: Performans Optimizasyonu
```
"State sync latency'yi 5ms'nin altına düşür"
"Simulation throughput'u 2x artır"
```

### Senaryo 4: Full Deploy Pipeline
```
"Production için release build hazırla"
"Yeni versiyonu test et ve deploy et"
```

## OTONOM İŞ AKIŞI

```
1. DISCOVERY (Otomatik)
   ├── Mevcut kod analizi (cargo check)
   ├── Bağımlılık kontrolü (Cargo.lock)
   ├── Test durumu (cargo test --release)
   └── Clippy analizi (cargo clippy -- -D warnings)

2. PLANNING
   ├── Değişiklik kapsamı belirleme
   ├── Etkilenen modüller tespiti
   └── Risk değerlendirmesi

3. IMPLEMENTATION
   ├── Zero-copy prensipleri
   ├── Async parallelization (FuturesUnordered)
   ├── Lock-free hot path (arc-swap)
   ├── Type-safe errors (thiserror/eyre)
   └── NO unwrap()/expect() kuralı

4. VALIDATION
   ├── cargo check
   ├── cargo clippy -- -D warnings
   ├── cargo test --release
   ├── cargo test --release -- --ignored proptest
   └── cargo build --release

5. INTEGRATION
   ├── REVM simülasyon doğrulama
   ├── Contract entegrasyon testi
   └── Shadow mode validasyonu

6. DEPLOY
   ├── Release binary build
   ├── .env validation
   └── Health check
```

## PROJE BAĞLAMI (Sabit Değerler)

| Parametre | Değer |
|-----------|-------|
| Rust Edition | 2021 |
| alloy | 1.7.x |
| revm | 36.x |
| tokio | 1.x (full) |
| Error handling | eyre 0.6.x |
| Locking | parking_lot 0.12.x |
| State sharing | arc-swap 1.x |

## MODÜL HARİTASI

```
Bot/src/
├── main.rs            → Entry point, event loop
├── types.rs           → PoolConfig, BotConfig, SharedPoolState
├── math.rs            → PreFilter, Newton-Raphson, exact U256
├── state_sync.rs      → Multicall3, slot0/liquidity sync
├── simulator.rs       → REVM simulation engine
├── strategy.rs        → Arbitrage detection, calldata encoding
├── executor.rs        → MevExecutor, TX submission
├── transport.rs       → IPC/WSS/HTTP provider
├── key_manager.rs     → AES-256-GCM keystore
├── pool_discovery.rs  → Factory event parsing
├── discovery_engine.rs→ Autonomous pool discovery
├── route_engine.rs    → Multi-hop route finding
├── json_logger.rs     → Shadow analytics logging
├── dust_sweeper.rs    → Small balance cleanup
└── telegram.rs        → Notification integration
```

## KOD ŞABLONLARI

### Yeni Fonksiyon Ekleme

```rust
/// Brief description of what this function does.
///
/// # Arguments
/// * `pools` - Slice reference (zero-copy)
///
/// # Returns
/// * `Result<T>` - Type-safe error handling
///
/// # Errors
/// Returns error if [condition].
pub async fn new_function(pools: &[PoolConfig]) -> eyre::Result<Output> {
    // 1. Validation
    if pools.is_empty() {
        return Err(eyre::eyre!("Empty pool list"));
    }

    // 2. Parallel processing
    let futs = FuturesUnordered::new();
    for pool in pools {
        futs.push(async_operation(pool));
    }

    let mut results = Vec::with_capacity(pools.len());
    while let Some(result) = futs.next().await {
        results.push(result?);
    }

    // 3. Return
    Ok(Output { results })
}
```

### Error Handling Pattern

```rust
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ModuleError {
    #[error("Pool {pool} data is stale: {staleness_ms}ms > {threshold_ms}ms")]
    StaleData {
        pool: Address,
        staleness_ms: u64,
        threshold_ms: u64,
    },

    #[error("RPC call failed")]
    RpcError(#[source] alloy::transports::TransportError),

    #[error("Simulation failed: {0}")]
    SimulationFailed(String),
}
```

### Async Parallelization

```rust
use futures_util::stream::FuturesUnordered;

// ✅ DOĞRU: Parallel execution
let mut futs = FuturesUnordered::new();
for item in items {
    futs.push(process_item(item));
}

while let Some(result) = futs.next().await {
    handle_result(result?)?;
}

// ❌ YASAK: Sequential (latency katlanır)
for item in items {
    let result = process_item(item).await; // N × latency
}
```

### Lock-Free State Management

```rust
use arc_swap::ArcSwap;

// State tanımlama
pub type SharedState = ArcSwap<Arc<State>>;

// Okuma (lock-free, ~1ns)
let current = shared_state.load();

// Yazma (atomik swap)
shared_state.store(Arc::new(new_state));
```

## TEST PROTOKOLÜ

### Otomatik Test Sırası

```bash
# 1. Syntax check (~2s)
cargo check

# 2. Lint check (~5s)
cargo clippy -- -D warnings

# 3. Unit tests (~10s)
cargo test --release

# 4. Property tests (~30s)
cargo test --release -- --ignored proptest

# 5. Release build (~60s)
cargo build --release
```

### Minimum Coverage Hedefleri

| Modül | Coverage |
|-------|----------|
| math.rs | 95% |
| simulator.rs | 90% |
| strategy.rs | 85% |
| executor.rs | 80% |
| Diğerleri | 70% |

## ÇIKIŞ RAPORU

```
═══════════════════════════════════════════════════════════════
🤖 AUTONOMOUS BOT DEVELOPMENT REPORT
═══════════════════════════════════════════════════════════════
Görev: [task description]
Durum: ✅ BAŞARILI | ❌ BAŞARISIZ
Süre: X.Xs

📁 DEĞİŞİKLİKLER:
├── Bot/src/[module].rs — [change summary]
├── Bot/Cargo.toml — [if changed]
└── Bot/src/types.rs — [if changed]

🧪 TEST SONUÇLARI:
├── cargo check:     ✅
├── cargo clippy:    ✅ (0 warnings)
├── cargo test:      ✅ (X/X passed)
├── proptest:        ✅ (Y cases)
└── release build:   ✅ (binary size: Z MB)

⚡ PERFORMANS:
├── Before: Xms
├── After:  Yms
└── İyileşme: Z%

🔗 ENTEGRASYON:
├── REVM simulation: ✅ consistent
└── Contract compat: ✅ verified
═══════════════════════════════════════════════════════════════
```

## KRİTİK KONTROL LİSTESİ

- [ ] `unwrap()` / `expect()` yok (test dışında)
- [ ] Zero-copy: `&[T]`, `Cow`, `Arc` kullanıldı
- [ ] Async: `FuturesUnordered` ile paralelize
- [ ] Errors: Tip-güvenli (thiserror veya eyre)
- [ ] Hot path: `arc-swap` veya `parking_lot`
- [ ] Windows uyumlu path handling
- [ ] Tüm testler geçiyor
- [ ] Clippy warning yok

---

*"Mikrosaniyeler milyonları kurtarır."*
