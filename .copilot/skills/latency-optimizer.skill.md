---
name: "Latency Optimizer"
description: "Sub-100ms hedefi için low-latency optimizasyonları — profiling, hotpath analizi"
---

# ⚡ LATENCY OPTIMIZER SKILL

> **Amaç:** Arbitraj pipeline'ını sub-100ms'ye optimize et — pool sync, simulation, TX submission.

## KULLANIM SENARYOLARI

### Senaryo 1: Genel Latency Audit
```
"Latency analizi yap"
"Nerede yavaşız?"
```

### Senaryo 2: Spesifik Optimizasyon
```
"Pool sync'i 5ms'nin altına düşür"
"Simulation throughput'u artır"
```

### Senaryo 3: Regression Analizi
```
"Son değişiklik latency'yi artırdı mı?"
"Performance baseline karşılaştırması"
```

## LATENCY BUDGET

```
┌─────────────────────────────────────────────────────────────┐
│                   100ms LATENCY BUDGET                      │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  Pool Sync              ████░░░░░░░░░░░░░░░░  5ms   (5%)    │
│  State Processing       ██░░░░░░░░░░░░░░░░░░  2ms   (2%)    │
│  Route Discovery        ███░░░░░░░░░░░░░░░░░  3ms   (3%)    │
│  REVM Simulation        ████████████████████ 50ms  (50%)    │
│  Calldata Encoding      █░░░░░░░░░░░░░░░░░░░  1ms   (1%)    │
│  TX Signing             ██░░░░░░░░░░░░░░░░░░  2ms   (2%)    │
│  TX Submission          ████████░░░░░░░░░░░░ 20ms  (20%)    │
│  Network Overhead       ███████░░░░░░░░░░░░░ 17ms  (17%)    │
│                                                             │
│  TOTAL                                       100ms          │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

## PROFİLİNG ARAÇLARI

### Rust Profiling

```bash
# Flamegraph
cargo install flamegraph
cargo flamegraph --bin arbitraj_botu

# perf (Linux)
perf record -g ./target/release/arbitraj_botu
perf report

# Timing macros
RUST_LOG=trace cargo run --release
```

### Timing Measurement

```rust
use std::time::Instant;

let start = Instant::now();
// ... operation ...
let elapsed = start.elapsed();
tracing::debug!("[LATENCY] operation: {:?}", elapsed);
```

### Memory Profiling

```bash
# valgrind (Linux)
valgrind --tool=massif ./target/release/arbitraj_botu

# heaptrack
heaptrack ./target/release/arbitraj_botu
```

## OPTİMİZASYON TEKNİKLERİ

### 1. Pool Sync Optimizasyonu

```rust
// ❌ YAVAŞ: Sequential RPC calls
for pool in pools {
    let state = provider.call(&pool).await?;
}

// ✅ HIZLI: Multicall3 batch
let multicall = Multicall3::new(&provider);
for pool in pools {
    multicall.add_call(pool.slot0_call());
    multicall.add_call(pool.liquidity_call());
}
let results = multicall.call().await?; // Tek RPC call

// ✅ DAHA HIZLI: Parallel multicall batches
let batch_size = 50;
let mut futs = FuturesUnordered::new();
for chunk in pools.chunks(batch_size) {
    futs.push(multicall_batch(chunk));
}
```

### 2. REVM Simulation Optimizasyonu

```rust
// ❌ YAVAŞ: Her simülasyon için yeni DB
let db = InMemoryDB::new();
let result = simulate(&db, params);

// ✅ HIZLI: DB caching + state snapshot
let cached_db = CachedDB::new(base_state);
cached_db.apply_pending_state(); // Incremental update
let result = simulate(&cached_db, params);

// ✅ DAHA HIZLI: Parallel simulation
let sims = FuturesUnordered::new();
for opportunity in opportunities {
    let db_clone = cached_db.clone();
    sims.push(tokio::spawn(simulate(db_clone, opportunity)));
}
```

### 3. Lock-Free Hot Path

```rust
// ❌ YAVAŞ: Mutex on hot path
let state = mutex.lock().unwrap();
process(state);

// ✅ HIZLI: arc-swap (lock-free read)
let state = arc_swap.load(); // ~1ns
process(&state);

// ✅ DAHA HIZLI: Thread-local cache
thread_local! {
    static CACHED_STATE: RefCell<Option<Arc<State>>> = RefCell::new(None);
}
```

### 4. Memory Allocation Reduction

```rust
// ❌ YAVAŞ: Allocation in hot loop
for item in items {
    let vec = Vec::new();  // Allocation per iteration
    process(&vec);
}

// ✅ HIZLI: Pre-allocated buffer
let mut buffer = Vec::with_capacity(expected_size);
for item in items {
    buffer.clear();
    process(&mut buffer);
}

// ✅ DAHA HIZLI: Stack allocation
let mut buffer: [u8; 256] = [0; 256];
```

### 5. Network Latency Reduction

```rust
// ❌ YAVAŞ: HTTP RPC
let provider = ProviderBuilder::new().connect_http(url);

// ✅ HIZLI: WebSocket (persistent connection)
let provider = ProviderBuilder::new().connect_ws(wss_url).await?;

// ✅ EN HIZLI: IPC (same machine)
let provider = ProviderBuilder::new().connect_ipc(ipc_path).await?;
// Typical latency: HTTP ~50ms, WS ~10ms, IPC ~1ms
```

### 6. Calldata Encoding Optimization

```rust
// ❌ YAVAŞ: alloy abi encoding
let calldata = contract.function(params).abi_encode();

// ✅ HIZLI: Manual byte assembly
let mut calldata = Vec::with_capacity(134);
calldata.extend_from_slice(&pool_a.0);      // 20 bytes
calldata.extend_from_slice(&pool_b.0);      // 20 bytes
calldata.extend_from_slice(&amount.to_be_bytes::<32>()); // 32 bytes
// ...
```

## BENCHMARK FRAMEWORK

```rust
#[cfg(test)]
mod benchmarks {
    use std::time::{Duration, Instant};

    const ITERATIONS: usize = 1000;
    const WARMUP: usize = 100;

    fn benchmark<F: FnMut()>(name: &str, mut f: F) {
        // Warmup
        for _ in 0..WARMUP {
            f();
        }

        // Measure
        let start = Instant::now();
        for _ in 0..ITERATIONS {
            f();
        }
        let elapsed = start.elapsed();
        let per_op = elapsed / ITERATIONS as u32;

        println!("[BENCH] {}: {:?} per op ({} ops/sec)",
            name,
            per_op,
            1_000_000_000 / per_op.as_nanos()
        );
    }

    #[test]
    #[ignore]
    fn bench_pool_sync() {
        benchmark("pool_sync", || {
            // ...
        });
    }

    #[test]
    #[ignore]
    fn bench_simulation() {
        benchmark("revm_simulation", || {
            // ...
        });
    }
}
```

## LATENCY REGRESSION DETECTION

```rust
// CI/CD integration
const LATENCY_THRESHOLDS: &[(&str, Duration)] = &[
    ("pool_sync", Duration::from_millis(5)),
    ("simulation", Duration::from_millis(50)),
    ("encoding", Duration::from_millis(1)),
    ("total_cycle", Duration::from_millis(100)),
];

fn check_regression(results: &HashMap<&str, Duration>) -> bool {
    for (name, threshold) in LATENCY_THRESHOLDS {
        if let Some(actual) = results.get(name) {
            if actual > threshold {
                eprintln!("REGRESSION: {} = {:?} > {:?}",
                    name, actual, threshold);
                return true;
            }
        }
    }
    false
}
```

## INLINE ANNOTATIONS

```rust
// Hot path fonksiyonları
#[inline(always)]
fn encode_address(addr: &Address, buffer: &mut [u8]) {
    buffer.copy_from_slice(&addr.0);
}

// Nadiren çağrılan error paths
#[cold]
#[inline(never)]
fn handle_error(e: &Error) {
    // ...
}

// Branch prediction hints
#[likely]
if success {
    process();
}

#[unlikely]
if rare_error {
    handle();
}
```

## ÇIKIŞ RAPORU

```
═══════════════════════════════════════════════════════════════
⚡ LATENCY OPTIMIZATION REPORT
═══════════════════════════════════════════════════════════════
Zaman: [timestamp]
Hedef: < 100ms total cycle
Durum: ✅ HEDEFE ULAŞILDI | ⚠️ HEDEFE YAKIN | ❌ HEDEF AŞILDI

📊 LATENCY BREAKDOWN:

┌─────────────────────────────────────────────────────────────┐
│ COMPONENT           │ BEFORE   │ AFTER    │ DELTA          │
├─────────────────────┼──────────┼──────────┼────────────────┤
│ Pool Sync           │  8.2ms   │  3.1ms   │ -5.1ms (-62%)  │
│ State Processing    │  3.5ms   │  1.8ms   │ -1.7ms (-49%)  │
│ Route Discovery     │  4.1ms   │  2.9ms   │ -1.2ms (-29%)  │
│ REVM Simulation     │ 72.0ms   │ 45.0ms   │ -27.0ms (-38%) │
│ Calldata Encoding   │  1.2ms   │  0.8ms   │ -0.4ms (-33%)  │
│ TX Submission       │ 25.0ms   │ 18.0ms   │ -7.0ms (-28%)  │
├─────────────────────┼──────────┼──────────┼────────────────┤
│ TOTAL               │ 114.0ms  │  71.6ms  │ -42.4ms (-37%) │
└─────────────────────────────────────────────────────────────┘

🔧 UYGULANAN OPTİMİZASYONLAR:

1. Pool Sync
   ├── Multicall3 batch size: 20 → 50
   └── Parallel batches: enabled

2. REVM Simulation
   ├── DB caching: implemented
   └── State snapshots: enabled

3. TX Submission
   └── HTTP → WebSocket: migrated

📈 PERFORMANCE TREND:
├── 7-day avg: 89ms
├── 24h avg:   75ms
└── Current:   72ms

═══════════════════════════════════════════════════════════════
```

## KRİTİK KONTROL LİSTESİ

- [ ] Hot path'te allocation yok
- [ ] Mutex yerine arc-swap/atomic
- [ ] FuturesUnordered ile parallelization
- [ ] IPC/WebSocket tercih edilmiş (HTTP değil)
- [ ] Multicall3 batch optimization
- [ ] REVM DB caching aktif
- [ ] #[inline(always)] kritik fonksiyonlarda
- [ ] Benchmark regression test var

---

*"Her milisaniye, bin dolardır."*
