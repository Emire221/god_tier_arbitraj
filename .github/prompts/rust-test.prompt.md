---
agent: agent
description: "Rust modülü için kapsamlı unit ve integration testleri yazar"
---

# 🧪 Rust Test Yazımı

## Görev
Seçili Rust kodu/modülü için **kapsamlı test suite** oluştur.

## Test Kategorileri

### 1. Unit Tests
- Her public fonksiyon için
- Edge case'ler (zero, max, overflow)
- Error path'ler

### 2. Integration Tests
- Modüller arası etkileşim
- Async akışlar
- External dependency mock'ları

### 3. Property-Based Tests (proptest)
- Matematiksel invariant'lar
- Fuzz testing
- Boundary conditions

## Test Şablonu

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // ===== UNIT TESTS =====

    #[test]
    fn test_function_name_success_case() {
        // Arrange
        let input = ...;

        // Act
        let result = function_name(input);

        // Assert
        assert_eq!(result, expected);
    }

    #[test]
    fn test_function_name_error_case() {
        // Arrange
        let invalid_input = ...;

        // Act
        let result = function_name(invalid_input);

        // Assert
        assert!(result.is_err());
        assert!(matches!(result, Err(ErrorType::SpecificError)));
    }

    // ===== ASYNC TESTS =====

    #[tokio::test]
    async fn test_async_function() {
        // ...
    }

    // ===== PROPERTY TESTS =====

    use proptest::prelude::*;

    proptest! {
        #[test]
        fn test_invariant(input in 0u64..1000) {
            // Property assertion
            prop_assert!(invariant_holds(input));
        }
    }
}
```

## Arbitraj-Spesifik Test Senaryoları

### Pool State Tests
```rust
#[test]
fn test_pool_state_staleness_detection() {
    let stale_state = PoolState {
        last_update: Instant::now() - Duration::from_secs(10),
        ..Default::default()
    };
    assert!(stale_state.is_stale(Duration::from_secs(5)));
}
```

### Profit Calculation Tests
```rust
#[test]
fn test_profit_calculation_precision() {
    // Wei-level precision test
    let input = U256::from(1_000_000_000_000_000_000u128); // 1 ETH
    let output = calculate_output(input);
    assert!(output > input, "Arbitrage should be profitable");
}
```

### Simulation Tests
```rust
#[test]
fn test_revm_simulation_matches_onchain() {
    // Local simulation vs expected on-chain result
}
```

## Çalıştırma Komutları
```bash
# Tüm testler
cargo test --release

# Belirli test
cargo test test_function_name

# Property tests
cargo test --release -- --ignored proptest
```

## Kısıtlamalar
- Test içinde `unwrap()` ve `expect()` **İZİNLİ**
- Mock'lar için `mockall` crate kullan
- Async testler için `tokio::test` attribute
