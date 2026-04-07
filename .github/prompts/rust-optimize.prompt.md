---
agent: agent
description: "Rust kodunu performans açısından optimize eder - zero-copy, async parallelization, memory efficiency"
---

# 🚀 Rust Performans Optimizasyonu

## Görev
Seçili Rust kodunu analiz et ve **düşük gecikme (low-latency)** prensiplerine göre optimize et.

## Analiz Kontrol Listesi

### 1. Bellek Yönetimi (Zero-Copy)
- [ ] Gereksiz `.clone()` kullanımı var mı?
- [ ] `&T` yerine `T` ownership alınıyor mu?
- [ ] `Cow<'_, T>` kullanılabilir mi?
- [ ] `Arc<T>` ile paylaşımlı sahiplik uygun mu?
- [ ] `Bytes::copy_from_slice()` yerine `Bytes::from_static()` kullanılabilir mi?

### 2. Async Optimizasyonu
- [ ] Loop içinde sıralı `.await` var mı? → `FuturesUnordered` kullan
- [ ] CPU-bound işler `tokio::spawn_blocking` ile ayrılmış mı?
- [ ] `tokio::select!` ile timeout mekanizması var mı?
- [ ] `parking_lot::{Mutex, RwLock}` kullanılıyor mu?

### 3. Allocation Minimizasyonu
- [ ] Hot path'te `Vec::new()` yerine `Vec::with_capacity()` kullanılıyor mu?
- [ ] String concatenation için `format!` yerine `push_str` tercih edilmiş mi?
- [ ] `SmallVec` veya `ArrayVec` uygun mu?

## Optimizasyon Şablonları

### Sıralı Await → Paralel
```rust
// ❌ ÖNCE (N × latency)
for pool in pools {
    let state = fetch_pool_state(pool).await;
}

// ✅ SONRA (max(latency))
use futures_util::stream::FuturesUnordered;
let mut futs = FuturesUnordered::new();
for pool in pools {
    futs.push(fetch_pool_state(pool));
}
while let Some(result) = futs.next().await {
    handle(result)?;
}
```

### Clone → Reference
```rust
// ❌ ÖNCE
fn process(data: Vec<u8>) { ... }

// ✅ SONRA
fn process(data: &[u8]) { ... }
```

## Çıktı Formatı
1. **Mevcut Durum**: Tespit edilen sorunlar
2. **Önerilen Değişiklikler**: Kod örnekleriyle
3. **Beklenen İyileşme**: Latency/memory impact tahmini

## Kısıtlamalar
- `unwrap()` ve `expect()` YASAK
- `ethers-rs` DEĞİL, `alloy` kullan
- `std::sync::Mutex` yerine `parking_lot` tercih et
