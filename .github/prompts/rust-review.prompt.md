---
agent: agent
description: "Rust kodunu anayasa kurallarına göre inceler - unwrap yasağı, error handling, best practices"
---

# 🔍 Rust Kod İnceleme

## Görev
Seçili Rust kodunu **God Tier Arbitraj Anayasası**na göre incele ve sorunları raporla.

## Kritik Kural Kontrolleri

### 🚨 MUTLAK YASAK (Blocking)
- [ ] `.unwrap()` kullanımı (test dışında)
- [ ] `.expect()` kullanımı (test dışında)
- [ ] `ethers::` import'u (alloy kullanılmalı)
- [ ] `panic!()` production kodunda

### ⚠️ UYARI (Warning)
- [ ] `std::sync::Mutex` (parking_lot tercih et)
- [ ] Loop içinde `.clone()`
- [ ] Sıralı `.await` pattern
- [ ] Hardcoded timeout değerleri

### ✅ BEST PRACTICE
- [ ] `thiserror` veya `eyre` ile tip-güvenli hatalar
- [ ] `#[instrument]` ile tracing
- [ ] `const` yerine `static` tercihi
- [ ] Documentation comments (`///`)

## İnceleme Çıktısı

### Sorunlar Tablosu
| Satır | Seviye | Sorun | Öneri |
|-------|--------|-------|-------|
| ? | 🚨/⚠️/✅ | Açıklama | Çözüm |

### Düzeltme Önerileri
Her sorun için kod örneği ile düzeltme öner.

## Hata Yönetimi Kontrolü

```rust
// ❌ YANLIŞ
let value = result.unwrap();

// ✅ DOĞRU - Propagation
let value = result?;

// ✅ DOĞRU - Explicit error
let value = result.map_err(|e| ArbitrageError::ParseFailed(e))?;

// ✅ DOĞRU - Default fallback
let value = result.unwrap_or_default();
```

## Proje Bağlamı
- **Crate**: alloy 1.7.x, revm 36.x, tokio
- **Hedef**: Sub-100ms latency
- **Platform**: Windows uyumlu
