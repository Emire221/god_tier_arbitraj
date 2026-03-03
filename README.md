# God-Tier Arbitraj — Base Network Flash Swap Arbitrage System

> **Kuantum Beyin III** — Sub-millisecond cross-DEX arbitrage engine for Base L2, combining a Rust off-chain bot with an ultra-optimized Solidity smart contract.

```
 ╔══════════════════════════════════════════════════════════════╗
 ║       ARBITRAJ BOTU v9.0 — Kuantum Beyin III                ║
 ║       Base Network Çapraz-DEX Arbitraj Sistemi              ║
 ╠══════════════════════════════════════════════════════════════╣
 ║  54 Rust Tests ✓  58 Solidity Tests ✓  Chaos Injector ✓    ║
 ╚══════════════════════════════════════════════════════════════╝
```

---

## Table of Contents

- [Architecture Overview](#architecture-overview)
- [How the Bot Works (Rust)](#how-the-bot-works-rust)
- [How the Contract Works (Solidity)](#how-the-contract-works-solidity)
- [134-Byte Compact Calldata Format](#134-byte-compact-calldata-format)
- [Security Model](#security-model)
- [Test Suite — 112 Tests](#test-suite--112-tests)
  - [Rust Tests (54)](#rust-tests-54)
  - [Solidity Tests (58)](#solidity-tests-58)
- [Chaos Injector — End-to-End Hell Simulation](#chaos-injector--end-to-end-hell-simulation)
- [Project Structure](#project-structure)
- [Quick Start](#quick-start)

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│                        BASE L2 NETWORK                              │
│                                                                     │
│   ┌──────────────┐          ┌──────────────┐                        │
│   │ Uniswap V3   │◄────────│ Aerodrome    │                        │
│   │   Pool A     │  price   │ Slipstream   │                        │
│   │  (WETH/USDC) │  spread  │   Pool B     │                        │
│   └──────┬───────┘          └──────┬───────┘                        │
│          │                         │                                │
│          │    ┌─────────────────┐   │                                │
│          └────┤  ArbitrajBotu   ├───┘                                │
│               │  Contract v9.0 │                                    │
│               │  (134B Calldata│                                    │
│               │   Flash Swap)  │                                    │
│               └────────┬───────┘                                    │
│                        │                                            │
└────────────────────────┼────────────────────────────────────────────┘
                         │  TX (134 bytes)
                         │
┌────────────────────────┼────────────────────────────────────────────┐
│  OFF-CHAIN BOT (Rust)  │                                            │
│                        │                                            │
│  ┌──────────┐  ┌───────┴────┐  ┌────────────┐  ┌───────────────┐   │
│  │  State   │  │  Strategy  │  │  REVM      │  │  Key Manager  │   │
│  │  Sync    │──│  Engine    │──│  Simulator │  │  (AES-256)    │   │
│  │(Multicall│  │(Newton-    │  │(Local EVM) │  │               │   │
│  │ + Bitmap)│  │ Raphson)   │  │            │  │               │   │
│  └──────────┘  └────────────┘  └────────────┘  └───────────────┘   │
│                                                                     │
│  Transport Priority: IPC > WSS > HTTP (Sub-1ms target)             │
└─────────────────────────────────────────────────────────────────────┘
```

**Execution Flow (per block):**

1. **State Sync** — Multicall3 batch reads `slot0` + `liquidity` from both pools in a single RPC call
2. **TickBitmap Sync** — Off-chain bitmap snapshot for multi-tick depth simulation
3. **Price Calculation** — `sqrtPriceX96` → ETH/USDC price with tick cross-validation
4. **Opportunity Detection** — Spread analysis + dynamic gas cost (REVM-fed)
5. **Newton-Raphson Optimization** — Optimal trade size with multi-tick awareness
6. **REVM Simulation** — Local EVM execution to verify profitability + get exact gas
7. **TX Submission** — 134-byte compact calldata with deadline + bribe

---

## How the Bot Works (Rust)

**Crate:** `arbitraj_botu v9.0` — 6 modules, ~5000 lines of Rust

### Modules

| Module | Lines | Purpose |
|--------|-------|---------|
| `main.rs` | 910 | Entry point, block loop, reconnect logic, transport selection |
| `types.rs` | 959 | All data structures: `BotConfig`, `PoolState`, `SharedPoolState`, `SimulationResult` |
| `math.rs` | 2547 | AMM math engine: multi-tick swaps, Newton-Raphson optimizer, dampening |
| `state_sync.rs` | 1080+ | RPC pool state synchronization via Multicall3, TickBitmap management |
| `simulator.rs` | 1050+ | REVM-based local EVM simulation, calldata encoding/decoding |
| `strategy.rs` | 1226 | Opportunity detection, dynamic gas cost, TX building & submission |
| `key_manager.rs` | — | AES-256-GCM encrypted private key management with PBKDF2 |

### Key Features

- **Multi-Transport:** Auto-selects IPC → WSS → HTTP for lowest latency
- **REVM Simulation:** Zero-latency local EVM execution replaces `eth_call` RPC
- **Dynamic Gas:** Previous REVM simulation's gas → next block's gas cost estimate (no hardcoded values)
- **TickBitmap Depth:** Real multi-tick swap simulation using on-chain bitmap snapshots
- **Newton-Raphson:** Optimal trade size maximizing profit minus gas minus flash loan fee
- **Circuit Breaker:** Consecutive failure threshold stops execution to prevent capital loss
- **Shadow Mode:** Log-only mode for dry-run testing against live data
- **Encrypted Keys:** AES-256-GCM + PBKDF2 key storage (never plaintext on disk)

### Block Processing Pipeline

```
New Block Header (via subscription)
       │
       ▼
┌─────────────────┐     ┌──────────────────┐
│ sync_pool_state │────▶│ check_arbitrage  │
│ (Multicall3)    │     │ _opportunity()   │
│ Pool A + Pool B │     │ spread > min?    │
│ slot0+liquidity │     │ gas cost ok?     │
└─────────────────┘     │ NR optimal size  │
                        └────────┬─────────┘
                                 │ Some(opportunity)
                                 ▼
                        ┌──────────────────┐
                        │ evaluate_and_    │
                        │ execute()        │
                        │ REVM simulate    │
                        │ Build 134B TX    │
                        │ Sign + Submit    │
                        └──────────────────┘
```

### Dynamic Gas Cost Formula

```
gas_cost_usd = (last_revm_gas × block_base_fee) / 1e18 × eth_price_usd
```

- `last_revm_gas`: Actual gas from previous REVM simulation (fallback: 150K for first block)
- `block_base_fee`: Current block's EIP-1559 base fee
- If `base_fee == 0`: Falls back to `config.gas_cost_usd` (pre-EIP-1559 compatibility)

### Reconnect Logic

Exponential backoff: Immediate → 100ms × 3 → 200ms → 400ms → 800ms → ... → 10s cap

---

## How the Contract Works (Solidity)

**Contract:** `ArbitrajBotu v9.0` — 508 lines, Solidity `^0.8.27`, Cancun EVM

### Design Philosophy

- **Zero ABI encoding** — Raw `calldataload` via assembly (saves ~2000 gas)
- **No function selectors** — Single `fallback()` entry point
- **No on-chain storage reads** — Pool addresses come from calldata (saves ~4200 gas)
- **EIP-1153 Transient Storage** — Callback context passes through `TSTORE`/`TLOAD`
- **Immutable roles** — `executor` and `admin` stored in bytecode (~3 gas per read)

### Execution Flow

```
         Bot sends 134 bytes
              │
              ▼
┌─────────── fallback() ────────────┐
│  1. executor == msg.sender?       │
│  2. Reentrancy lock (TSTORE 0xFF)│
│  3. Parse calldata (assembly)     │
│  4. Deadline check                │
│  5. TSTORE callback context       │
│  6. balanceBefore = balanceOf()   │
│  7. UniV3.swap() ─── FLASH ──┐   │
│                               │   │
│  ┌── uniswapV3SwapCallback ◄─┘   │
│  │  Path A: msg.sender==UniV3     │
│  │    → Slipstream.swap() ──┐     │
│  │                          │     │
│  │  ┌─ callback (Path B) ◄──┘    │
│  │  │  msg.sender==Slipstream     │
│  │  │  → Pay receivedToken debt   │
│  │  └────────────────────────     │
│  │                                │
│  │  → Pay owedToken to UniV3      │
│  └────────────────────────────    │
│                                   │
│  8. balanceAfter = balanceOf()    │
│  9. profit = after - before       │
│ 10. profit >= minProfit?          │
│ 11. Emit ArbitrageExecuted       │
└───────────────────────────────────┘
```

### Role Separation

| Role | Permissions | Key Type |
|------|-------------|----------|
| `executor` | Call `fallback()` only | Hot wallet (low balance) |
| `admin` | Call `withdrawToken()`, `withdrawETH()` only | Cold wallet / Multisig |

If the executor key is compromised, the attacker can only execute arbitrage — they **cannot** withdraw accumulated profits.

### Gas Profile

| Operation | Gas Cost |
|-----------|----------|
| Successful arbitrage (typical) | ~115,000 |
| calldata parsing (assembly) | ~200 |
| TSTORE/TLOAD context | ~500 |
| UniV3 flash swap | ~80,000 |
| Slipstream swap | ~30,000 |

---

## 134-Byte Compact Calldata Format

```
Offset   Size    Field                  Type
──────────────────────────────────────────────────
0x00     20 B    Pool A (UniV3)         address
0x14     20 B    Pool B (Slipstream)    address
0x28     20 B    owedToken              address
0x3C     20 B    receivedToken          address
0x50     32 B    amount                 uint256
0x70      1 B    uniDirection           uint8
0x71      1 B    aeroDirection          uint8
0x72     16 B    minProfit              uint128
0x82      4 B    deadlineBlock          uint32
──────────────────────────────────────────────────
TOTAL   134 B    (no ABI encoding overhead)
```

Standard ABI encoding for the same data: **292+ bytes**. Savings: **54%**.

---

## Security Model

### On-Chain Protections

| Protection | Mechanism | Attack Prevented |
|------------|-----------|-----------------|
| **Sandwich Protection** | `profit >= minProfit` check | MEV sandwich attacks |
| **Deadline Block** | `block.number <= deadlineBlock` | Stale TX exploitation |
| **Reentrancy Guard** | EIP-1153 transient storage lock | Reentrancy attacks |
| **Callback Validation** | `msg.sender == expectedPool` via TLOAD | Fake callback injection |
| **Role Separation** | Immutable executor/admin split | Key compromise → no fund loss |
| **Calldata Length** | Exactly 134 bytes or revert | Malformed calldata attacks |

### Off-Chain Protections

| Protection | Mechanism | Attack Prevented |
|------------|-----------|-----------------|
| **Mathematical Validation** | Staleness, price range, liquidity checks | Stale/phantom data |
| **Circuit Breaker** | N consecutive failures → halt | Cascading losses |
| **Dynamic Gas** | REVM-fed gas estimation | Gas spike losses |
| **Encrypted Keys** | AES-256-GCM + PBKDF2 | Key theft from disk |
| **Shadow Mode** | Dry-run without TX submission | Strategy bugs |

---

## Test Suite — 112 Tests

**54 Rust unit tests + 58 Solidity tests = 112 total tests**

All tests pass: `cargo test` ✓ | `forge test` ✓

### Rust Tests (54)

#### Math Engine (22 tests)

Core AMM math: swap simulations, price calculations, optimizer convergence.

| Test | Module | Purpose |
|------|--------|---------|
| `test_compute_eth_price_token0_weth` | math | sqrtPriceX96 → ETH price (token0=WETH) |
| `test_compute_eth_price_various` | math | Price accuracy across 1500-5000 range |
| `test_swap_weth_to_usdc_token0_weth` | math | 1 WETH → ~2000 USDC dampened swap |
| `test_swap_usdc_to_weth_token0_weth` | math | 2000 USDC → ~1 WETH reverse swap |
| `test_large_swap_dampening` | math | Price impact: large > small swap |
| `test_max_safe_swap_amount` | math | Liquidity-capped max trade size |
| `test_tick_price_roundtrip` | math | tick → price → tick lossless conversion |
| `test_newton_raphson_tick_aware` | math | NR optimizer convergence with tick awareness |
| `test_newton_raphson_with_bitmap` | math | NR optimizer with real TickBitmap data |
| `test_multitick_swap_with_bitmap` | math | Multi-tick swap using TickBitmap engine |
| `test_multitick_swap_tick_crossings_detail` | math | Tick crossing path verification |
| `stres_compute_eth_price` | math | Stress: ETH price across extreme ranges |
| `stres_dampening_sifir_likidite` | math | Stress: Zero liquidity edge case |
| `stres_max_safe_swap_amount` | math | Stress: Max swap with extreme inputs |
| `stres_multitick_with_bitmap` | math | Stress: Multi-tick with large amounts |
| `stres_sqrt_price_x96_to_tick` | math | Stress: Price-to-tick edge cases |
| `stres_swap_usdc_to_weth` | math | Stress: USDC→WETH with extreme values |
| `stres_swap_weth_to_usdc` | math | Stress: WETH→USDC with extreme values |
| `stres_tick_to_price_ratio` | math | Stress: Tick-to-price ratio boundaries |
| `test_compute_swap_step_basic` | math::exact | Exact integer swap step (U256) |
| `test_exact_swap_no_bitmap` | math::exact | Exact swap without bitmap |
| `test_exact_swap_with_bitmap` | math::exact | Exact swap with bitmap data |
| `test_get_sqrt_ratio_at_tick_zero` | math::exact | tick=0 → sqrtRatio correctness |
| `test_get_sqrt_ratio_negative_tick` | math::exact | Negative tick handling |
| `test_get_sqrt_ratio_at_tick_boundaries` | math::exact | MIN/MAX tick boundaries |
| `test_mul_div_basic` | math::exact | MulDiv basic arithmetic |
| `test_mul_div_large_numbers` | math::exact | MulDiv with U256 large numbers |

#### Calldata Encoding (8 tests)

134-byte compact calldata construction and validation.

| Test | Module | Purpose |
|------|--------|---------|
| `test_compact_calldata_is_134_bytes` | simulator | Exact 134 byte output |
| `test_compact_calldata_byte_layout` | simulator | Field positions match Solidity offsets |
| `test_compact_calldata_encode_decode_roundtrip` | simulator | Encode → decode → identical values |
| `test_compact_calldata_invalid_length_rejected` | simulator | Reject non-134 byte inputs |
| `test_compact_vs_abi_size_comparison` | simulator | 134B vs 292B ABI comparison |
| `test_format_compact_calldata_hex` | simulator | Hex string formatting |
| `test_min_profit_max_u128` | simulator | uint128 max value encoding |
| `test_real_base_scenario` | simulator | Real Base mainnet addresses |

#### L2 Sequencer Reorg Protection (5 tests)

Validates bot behavior during Base sequencer instability.

| Test | Module | Purpose |
|------|--------|---------|
| `test_sequencer_reorg_handling` | simulator | Stale state (>5s) → opportunity rejected |
| `test_sequencer_reorg_phantom_opportunity` | simulator | Zero sqrt_price (uninitialized) → rejected |
| `test_sequencer_full_outage_both_pools_stale` | simulator | Both pools stale → no false positive |
| `test_fresh_state_passes_validation` | simulator | Fresh state → validation passes (positive control) |
| `test_sequencer_reorg_abnormal_price` | simulator | Price >100K → anomaly detection |

#### Gas Spike Resilience (3 tests)

Dynamic gas cost under extreme EIP-1559 base fee conditions.

| Test | Module | Purpose |
|------|--------|---------|
| `test_circuit_breaker_on_gas_spike` | strategy | 500K Gwei spike → opportunity rejected ($187 cost) |
| `test_gas_spike_large_spread_still_profitable` | strategy | 2% spread survives 500 Gwei spike |
| `test_zero_base_fee_uses_config_fallback` | strategy | base_fee=0 → config.gas_cost_usd fallback |

#### RPC Connection Failover (5 tests)

State resilience during RPC disconnections and reconnects.

| Test | Module | Purpose |
|------|--------|---------|
| `test_rpc_failover_without_panic` | state_sync | RPC drop → state preserved, no panic |
| `test_rpc_consecutive_failures_staleness_protection` | state_sync | Old state marked stale after disconnect |
| `test_rpc_never_connected_no_panic` | state_sync | Default state (never synced) is safe |
| `test_rpc_failover_concurrent_access_no_panic` | state_sync | Multi-reader RwLock under failure |
| `test_reconnect_exponential_backoff_calculation` | state_sync | Backoff: 100ms → 200ms → ... → 10s cap |

#### Key Manager (6 tests)

AES-256-GCM encrypted private key storage.

| Test | Module | Purpose |
|------|--------|---------|
| `test_encrypt_decrypt_roundtrip` | key_manager | Encrypt → decrypt → same key |
| `test_wrong_password_fails` | key_manager | Wrong password → decryption fails |
| `test_different_keys_produce_different_ciphertexts` | key_manager | No ciphertext collision |
| `test_corrupted_file_fails` | key_manager | Tampered file → error |
| `test_empty_key_manager` | key_manager | Empty state is safe |
| `test_env_var_fallback` | key_manager | ENV variable fallback path |

---

### Solidity Tests (58)

#### Compact Calldata (4 tests)

| Test | Purpose |
|------|---------|
| `test_compactCalldata_Is134Bytes` | Packed calldata is exactly 134 bytes |
| `test_compactCalldata_SuccessfulArbitrage` | Full arbitrage cycle with 134B calldata |
| `test_compactCalldata_ReverseDirection` | Reverse direction (token1 owed) works |
| `test_compactCalldata_EmitsEvent` | `ArbitrageExecuted` event emitted correctly |

#### Access Control (3 tests)

| Test | Purpose |
|------|---------|
| `test_accessControl_FallbackRevertsIfNotExecutor` | Non-executor → `Unauthorized()` |
| `test_accessControl_CallbackRevertsIfNotExpectedPool` | Wrong pool → `InvalidCaller()` |
| `test_accessControl_CallbackRevertsIfRandomContract` | Random address → `InvalidCaller()` |

#### Deadline Protection (4 tests)

| Test | Purpose |
|------|---------|
| `test_deadline_PassesAtExactBlock` | `block.number == deadline` → passes |
| `test_deadline_PassesWithFutureBlock` | `block.number < deadline` → passes |
| `test_deadline_RevertsIfExpired` | `block.number > deadline` → `DeadlineExpired()` |
| `test_deadline_ZeroAlwaysReverts` | `deadline=0` always reverts |

#### Sandwich Protection (5 tests)

| Test | Purpose |
|------|---------|
| `test_sandwichProtection_RevertsIfProfitBelowMinProfit` | Profit < minProfit → revert |
| `test_sandwichProtection_PassesWhenProfitEqualsMinProfit` | Profit == minProfit → passes |
| `test_sandwichProtection_PassesWhenProfitExceedsMinProfit` | Profit > minProfit → passes |
| `test_sandwichProtection_1WeiProfitBlockedByMinProfit` | 1 wei profit blocked |
| `test_sandwichProtection_ZeroMinProfitStillRequiresProfit` | minProfit=0 still needs profit>0 |

#### Role Separation (3 tests)

| Test | Purpose |
|------|---------|
| `test_rolesSeparation_ExecutorCannotWithdrawToken` | Executor → `withdrawToken()` reverts |
| `test_rolesSeparation_ExecutorCannotWithdrawETH` | Executor → `withdrawETH()` reverts |
| `test_rolesSeparation_AdminCannotExecute` | Admin → `fallback()` reverts |

#### Profit Validation (4 tests)

| Test | Purpose |
|------|---------|
| `test_profitValidation_RevertsIfNoProfit` | Zero profit → `NoProfitRealized()` |
| `test_profitValidation_ExactBreakeven_Reverts` | Breakeven → revert |
| `test_profitValidation_MinimalProfit_Passes` | 1 wei profit → passes |
| `test_profitValidation_LargeProfit` | Large profit → passes |

#### Full Cycle Integration (3 tests)

| Test | Purpose |
|------|---------|
| `test_fullCycle_ArbitrageAndWithdraw` | Arb → profit → admin withdraw |
| `test_fullCycle_BothDirections` | Forward + reverse direction cycles |
| `test_fullCycle_MultipleArbitrages` | Multiple sequential arbs accumulate |

#### Constructor & Immutables (5 tests)

| Test | Purpose |
|------|---------|
| `test_constructor_SetsImmutableExecutorAndAdmin` | executor/admin set correctly |
| `test_constructor_DifferentAddresses` | Different executor/admin works |
| `test_constructor_RevertsIfExecutorZero` | Zero executor → revert |
| `test_constructor_RevertsIfAdminZero` | Zero admin → revert |
| `test_constructor_RevertsIfBothZero` | Both zero → revert |

#### Transient Storage (2 tests)

| Test | Purpose |
|------|---------|
| `test_transientStorage_CallbackReadsCorrectContext` | TSTORE→TLOAD context passes correctly |
| `test_transientStorage_NoStateCorruption` | Sequential TXs don't leak state |

#### Withdrawal (6 tests)

| Test | Purpose |
|------|---------|
| `test_withdrawToken_FullBalance` | Admin withdraws all tokens |
| `test_withdrawToken_RevertsIfNotAdmin` | Non-admin → revert |
| `test_withdrawToken_RevertsIfZeroBalance` | Zero balance → revert |
| `test_withdrawETH` | Admin withdraws all ETH |
| `test_withdrawETH_RevertsIfNotAdmin` | Non-admin → revert |
| `test_withdrawETH_RevertsIfZeroBalance` | Zero balance → revert |

#### Miscellaneous (4 tests)

| Test | Purpose |
|------|---------|
| `test_profitStaysInContract` | Profit accumulates in contract (no auto-send) |
| `test_getBalance` | `getBalance()` view function works |
| `test_receiveETH` | Contract accepts ETH via `receive()` |
| `test_removed_NoPausedFunction` | No pause function exists (no governance risk) |
| `test_calldata_RevertsIfZeroAmount` | Zero amount → `ZeroAmount()` |
| `test_gasProfile_SuccessfulArbitrage` | Gas measurement: ~115K per arb |
| `test_immutable_ExecutorAndAdminSetInConstructor` | Immutable storage verification |

#### Fuzz Tests (8 tests, 256 runs each)

| Test | Purpose |
|------|---------|
| `testFuzz_NoWeiLeakage` | 100 iterations × 256 seeds: no dust accumulation |
| `testFuzz_Fallback` | Random calldata → no unexpected behavior |
| `testFuzz_Fallback_Unauthorized` | Random addresses → always `Unauthorized()` |
| `testFuzz_Fallback_ValidPools_NoProfit` | Valid pools, no profit → `NoProfitRealized()` |
| `testFuzz_InvalidCallback` | Random callback callers → always `InvalidCaller()` |
| `testFuzz_InvalidCallback_WithRandomDeltas` | Random deltas + random callers rejected |
| `testFuzz_Deadline_Variations` | Random deadlines → correct pass/fail |
| `testFuzz_SandwichProtection_MinProfitVariations` | Random minProfit → correct enforcement |

#### Adversarial MEV Tests (4 tests)

| Test | Purpose |
|------|---------|
| `test_RevertOnJITLiquidityAttack` | JIT liquidity reduces profit below minProfit → revert |
| `test_JITAttack_LowMinProfit_StillPasses` | Low minProfit survives JIT attack |
| `test_JITAttack_ZeroProfit_Reverts` | JIT reduces profit to zero → revert |
| `test_JITAttack_AttackerCannotCallFallback` | Attacker address ≠ executor → `Unauthorized()` |

---

## Chaos Injector — End-to-End Hell Simulation

**Script:** `Bot/chaos_injector.sh` — Live-fire adversarial stress testing against an Anvil fork.

### What It Does

The Chaos Injector creates a **hostile market environment** to verify the bot survives real-world adversarial conditions:

```
┌──────────────────────────────────────────────────────────┐
│              CHAOS INJECTOR v1.1                         │
│                                                          │
│   1. Fork Base mainnet via Anvil                         │
│   2. Randomly manipulate sqrtPriceX96 in both pools     │
│      (±1-5% per cycle via storage slot override)        │
│   3. Mine blocks to trigger bot's state sync            │
│   4. Monitor WETH balance: profit/loss/unchanged        │
│   5. KILL if bot ever loses money (ZARAR = fatal)       │
│                                                          │
│   Cycle: manipulate → wait → mine → check balance       │
│   Duration: Infinite loop (or MAX_CYCLES cap)           │
└──────────────────────────────────────────────────────────┘
```

### Attack Vectors Simulated

| Attack | Method | Expected Bot Response |
|--------|--------|----------------------|
| **Price Manipulation** | `anvil_setStorageAt` on slot0 | Detect spread, arb if profitable |
| **Sudden Price Crash** | -5% sqrtPriceX96 | Skip if spread too small |
| **Price Spike** | +5% sqrtPriceX96 | Arb in correct direction |
| **No Opportunity** | Equal manipulation both pools | Skip cycle (balance unchanged) |
| **Rapid Cycles** | 2s between manipulations | No race conditions or crashes |

### Running the Full Test Pipeline

```bash
# Full automated pipeline: Foundry fuzz → Rust tests → Anvil chaos
./run_all_tests.sh
```

**Pipeline Stages:**

| Stage | Command | What It Tests |
|-------|---------|---------------|
| 1. Foundry Fuzz | `forge test --fuzz-runs 10000` | Contract security (10K random scenarios) |
| 2. Rust Tests | `cargo test --release` | Math engine, state sync, strategy logic |
| 3. Anvil + Bot + Chaos | Anvil fork → Bot → Chaos Injector | End-to-end: bot in hostile market (60s) |

### Success Criteria

- **Stage 1:** All 58 Solidity tests pass with 10K fuzz runs
- **Stage 2:** All 54 Rust tests pass
- **Stage 3:** Bot WETH balance never decreases (ZARAR = immediate abort)

---

## Project Structure

```
god_tier_arbitraj/
├── README.md                    ← You are here
├── run_all_tests.sh             ← Full test pipeline script
│
├── Bot/                         ← Rust off-chain arbitrage bot
│   ├── Cargo.toml               ← Dependencies (alloy, revm, tokio, parking_lot...)
│   ├── chaos_injector.sh        ← Chaos Injection stress test script
│   └── src/
│       ├── main.rs              ← Entry point, block loop, transport selection
│       ├── types.rs             ← BotConfig, PoolState, SharedPoolState, SimulationResult
│       ├── math.rs              ← AMM math: multi-tick swaps, Newton-Raphson, dampening
│       ├── state_sync.rs        ← RPC sync: Multicall3, TickBitmap management
│       ├── simulator.rs         ← REVM simulation engine, calldata encoding
│       ├── strategy.rs          ← Opportunity detection, TX building, submission
│       └── key_manager.rs       ← AES-256-GCM encrypted key management
│
├── Contract/                    ← Solidity smart contract (Foundry)
│   ├── foundry.toml             ← Foundry config (Cancun EVM, optimizer 1M runs)
│   ├── src/
│   │   └── Arbitraj.sol         ← ArbitrajBotu v9.0 (508 lines, 134B calldata)
│   ├── test/
│   │   └── Arbitraj.t.sol       ← 58 tests (unit + fuzz + adversarial)
│   └── lib/
│       ├── forge-std/           ← Foundry test framework
│       └── aave-v3-core/        ← AAVE V3 (flash loan reference)
```

---

## Quick Start

### Prerequisites

- **Rust** ≥ 1.75 (with `cargo`)
- **Foundry** (`forge`, `cast`, `anvil`)
- **Python** 3.x (for Chaos Injector calculations)

### Build & Test

```bash
# 1. Compile and test the Rust bot
cd Bot
cargo test          # 54 tests

# 2. Compile and test the Solidity contract
cd Contract
forge test          # 58 tests

# 3. Run the full pipeline (fuzz + unit + chaos)
./run_all_tests.sh
```

### Configuration

Create `Bot/.env`:

```env
# RPC
RPC_WSS_URL=wss://base-mainnet.your-rpc.io
RPC_HTTP_URL=https://base-mainnet.your-rpc.io

# Contract
CONTRACT_ADDRESS=0x...
POOL_A_ADDRESS=0x...    # Uniswap V3 WETH/USDC
POOL_B_ADDRESS=0x...    # Aerodrome Slipstream WETH/USDC

# Keys (use key_manager for encrypted storage)
PRIVATE_KEY=0x...       # Executor hot wallet

# Strategy
MIN_NET_PROFIT_USD=0.50
GAS_COST_USD=0.10
MAX_TRADE_SIZE_WETH=50.0
SHADOW_MODE=true        # Set false for live execution
```

---

## Tech Stack

| Layer | Technology | Purpose |
|-------|------------|---------|
| Smart Contract | Solidity 0.8.27, Cancun EVM | On-chain arbitrage execution |
| Bot Runtime | Rust, Tokio | Async block processing |
| EVM Interface | Alloy | RPC calls, TX signing, types |
| Local Simulation | REVM | Zero-latency EVM execution |
| State Locking | parking_lot RwLock | Lock-free concurrent reads |
| Cryptography | AES-256-GCM, PBKDF2 | Key encryption |
| Testing | Foundry (Forge), cargo test | Fuzz + unit + integration |
| Chaos Testing | Anvil + Bash + Cast | Live-fork stress testing |

---

<p align="center"><b>Built for Base L2 — Optimized for speed, hardened by chaos.</b></p>
