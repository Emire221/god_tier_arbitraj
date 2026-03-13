# God-Tier Arbitraj — Base Network Flash Swap Arbitrage System

> **Kuantum Beyin IV** — Sub-millisecond cross-DEX arbitrage engine for Base L2, combining a Rust off-chain bot with an ultra-optimized Solidity smart contract.

```
 ╔══════════════════════════════════════════════════════════════╗
 ║       ARBITRAJ BOTU v25.0 — Kuantum Beyin IV                ║
 ║       Base Network Capraz-DEX Arbitraj Sistemi              ║
 ╠══════════════════════════════════════════════════════════════╣
 ║  45 Rust Tests ✓  69 Solidity Tests ✓  Chaos Injector ✓    ║
 ╚══════════════════════════════════════════════════════════════╝
```

---

## Table of Contents

- [Architecture Overview](#architecture-overview)
- [How the Bot Works (Rust)](#how-the-bot-works-rust)
- [How the Contract Works (Solidity)](#how-the-contract-works-solidity)
- [134-Byte Compact Calldata Format](#134-byte-compact-calldata-format)
- [Security Model](#security-model)
- [Test Suite — 114 Tests](#test-suite--114-tests)
  - [Rust Tests (45)](#rust-tests-45)
  - [Solidity Tests (69)](#solidity-tests-69)
- [Chaos Injector — End-to-End Hell Simulation](#chaos-injector--end-to-end-hell-simulation)
- [Dust Sweeper — Auto Token Cleanup](#dust-sweeper--auto-token-cleanup)
- [Structured JSON Logging](#structured-json-logging)
- [Project Structure](#project-structure)
- [Quick Start](#quick-start)
- [CLI Reference](#cli-reference)

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
│               │  Contract v25.0│                                    │
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
│  ┌──────────┐  ┌────────────┐  ┌────────────┐  ┌───────────────┐   │
│  │ Discovery│  │  Executor  │  │  Transport │  │ Pool Discovery│   │
│  │  Engine  │  │ (Private   │  │  Pool      │  │ (DexScreener) │   │
│  │(Factory  │  │  RPC/MEV)  │  │(IPC>WSS>   │  │               │   │
│  │ Events)  │  │            │  │ HTTP)      │  │               │   │
│  └──────────┘  └────────────┘  └────────────┘  └───────────────┘   │
│                                                                     │
│  ┌──────────┐  ┌────────────┐                                       │
│  │  JSON    │  │   Dust     │  Transport Priority:                  │
│  │  Logger  │  │  Sweeper   │  IPC > WSS > HTTP (Sub-1ms target)   │
│  │(.jsonl)  │  │(WETH swap) │                                       │
│  └──────────┘  └────────────┘                                       │
└─────────────────────────────────────────────────────────────────────┘
```

**Execution Flow (per block):**

1. **State Sync** — Multicall3 batch reads `slot0` + `liquidity` from all pools in a single RPC call
2. **TickBitmap Sync** — Off-chain bitmap snapshot for multi-tick depth simulation (range: 100 words)
3. **Price Calculation** — `sqrtPriceX96` → ETH/USDC price with tick cross-validation
4. **Opportunity Detection** — Spread analysis + dynamic gas cost (REVM-fed)
5. **Newton-Raphson Optimization** — Optimal trade size with multi-tick awareness
6. **REVM Simulation** — Local EVM execution to verify profitability + get exact gas
7. **TX Submission** — 134-byte compact calldata via Private RPC (MEV-protected)

---

## How the Bot Works (Rust)

**Crate:** `arbitraj_botu v25.0` — 14 modules, ~12,000 lines of Rust

### Modules

| Module | Purpose |
|--------|---------|
| `main.rs` | Entry point, block loop, reconnect logic, hot-reload, CLI |
| `types.rs` | All data structures: `BotConfig`, `PoolState`, `SharedPoolState`, `SimulationResult` |
| `math.rs` | AMM math engine: multi-tick swaps, Newton-Raphson optimizer, U256 exact math |
| `state_sync.rs` | RPC pool state synchronization via Multicall3, TickBitmap management |
| `simulator.rs` | REVM-based local EVM simulation, calldata encoding/decoding |
| `strategy.rs` | Opportunity detection, dynamic gas cost, TX building & submission |
| `key_manager.rs` | AES-256-GCM encrypted private key management with PBKDF2 |
| `discovery_engine.rs` | Multi-source pool discovery: Factory events, V2 filtering, API aggregation |
| `pool_discovery.rs` | DexScreener API integration, pair matching, JSON export |
| `transport.rs` | HFT-grade RPC connection pool (IPC > WSS > HTTP), health monitoring |
| `executor.rs` | Private RPC MEV protection, `eth_sendRawTransaction`, dynamic bribe |
| `route_engine.rs` | Liquidity graph builder, DFS multi-hop route generator |
| `json_logger.rs` | Structured JSONL logging with auto-rotation (50MB threshold) |
| `dust_sweeper.rs` | ERC-20 dust auto-sweep to WETH via Uniswap V3 SwapRouter |

### Key Features

- **Multi-Transport:** Auto-selects IPC → WSS → HTTP for lowest latency (HFT-grade connection pool)
- **REVM Simulation:** Zero-latency local EVM execution replaces `eth_call` RPC
- **Dynamic Gas:** Previous REVM simulation's gas → next block's gas cost estimate
- **TickBitmap Depth:** Real multi-tick swap simulation using on-chain bitmap snapshots (100 word range)
- **Newton-Raphson:** Optimal trade size maximizing profit minus gas minus flash loan fee
- **Circuit Breaker:** Consecutive failure threshold stops execution to prevent capital loss
- **Shadow Mode:** Log-only mode for dry-run testing against live data with full statistics
- **Encrypted Keys:** AES-256-GCM + PBKDF2 key storage (never plaintext on disk)
- **Auto Pool Discovery:** Factory event scanning + DexScreener API for real-time pair detection
- **V2 Pool Isolation:** Label-based blacklist + factory whitelist prevents V2 pool contamination
- **MEV Executor:** Private RPC only (`eth_sendRawTransaction`) — no public mempool exposure
- **L1 Data Fee Awareness:** Accounts for Base OP Stack L1 data posting costs in profit calculations
- **Hot Reload:** Runtime pool addition without bot restart (factory events + API discovery)
- **Micro-Profit Strategy:** Base L2 optimized minimum profit threshold (0.00003 WETH = ~$0.075)
- **Dust Sweeper:** CLI tool to convert ERC-20 dust to WETH via Uniswap V3
- **Structured Logging:** JSONL file output with auto-rotation for analysis and monitoring

### Block Processing Pipeline

```
New Block Header (via WSS subscription)
       │
       ▼
┌──────────────────────────────────────────────┐
│ PARALLEL: sync_pools ∥ L1_fee ∥ TickBitmap  │
│ (Single tokio::join! — max(RTT) latency)    │
└──────────────────────┬───────────────────────┘
                       │
                       ▼
              ┌──────────────────┐
              │ check_arbitrage  │
              │  _opportunity()  │
              │  spread > min?   │
              │  gas cost ok?    │
              │  NR optimal size │
              └────────┬─────────┘
                       │ Some(opportunity)
                       ▼
              ┌──────────────────┐
              │ evaluate_and_    │
              │ execute()        │
              │  REVM simulate   │
              │  Build 134B TX   │
              │  Sign + Submit   │
              │  via Private RPC │
              └──────────────────┘
```

### Dynamic Gas Cost Formula

```
gas_cost_usd = (last_revm_gas × block_base_fee) / 1e18 × eth_price_usd
```

- `last_revm_gas`: Actual gas from previous REVM simulation (fallback: 150K for first block)
- `block_base_fee`: Current block's EIP-1559 base fee
- If `base_fee == 0`: Falls back to `config.gas_cost_usd`

### Reconnect Logic

Exponential backoff with jitter: Immediate → 100ms × 3 → 200ms → 400ms → ... → 30s cap (random jitter up to 50% of delay, max 2s)

---

## How the Contract Works (Solidity)

**Contract:** `ArbitrajBotu v25.0` — 599 lines, Solidity `^0.8.27`, Cancun EVM

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
| **V2 Pool Isolation** | Label blacklist + factory whitelist | V2/V3 AMM confusion |
| **Private RPC** | Flashbots Protect (Base) | Public mempool frontrunning |
| **Dust Slippage** | 5% minimum output on sweeps | Price manipulation on dust |
| **Env Security** | `.env` excluded from git tracking | API key exposure |

---

## Test Suite — 114 Tests

**45 Rust unit tests + 69 Solidity tests = 114 total tests**

All tests pass: `cargo test` ✓ | `forge test` ✓

### Rust Tests (45)

#### Math Engine (13 tests)

Core AMM math: swap simulations, price calculations, optimizer convergence.

| Test | Module | Purpose |
|------|--------|---------|
| `test_compute_eth_price_token0_weth` | math | sqrtPriceX96 → ETH price (token0=WETH) |
| `test_compute_eth_price_various` | math | Price accuracy across 1500-5000 range |
| `test_newton_raphson_with_bitmap` | math | NR optimizer with real TickBitmap data |
| `stres_compute_eth_price` | math | Stress: ETH price across extreme ranges |
| `stres_tick_to_price_ratio` | math | Stress: Tick-to-price ratio boundaries |
| `test_get_sqrt_ratio_at_tick_zero` | math::exact | tick=0 → sqrtRatio correctness |
| `test_get_sqrt_ratio_at_tick_boundaries` | math::exact | MIN/MAX tick boundaries |
| `test_get_sqrt_ratio_negative_tick` | math::exact | Negative tick handling |
| `test_mul_div_basic` | math::exact | MulDiv basic arithmetic |
| `test_mul_div_large_numbers` | math::exact | MulDiv with U256 large numbers |
| `test_compute_swap_step_basic` | math::exact | Exact integer swap step (U256) |
| `test_exact_swap_no_bitmap` | math::exact | Exact swap without bitmap |
| `test_exact_swap_with_bitmap` | math::exact | Exact swap with bitmap data |

#### Calldata & Sequencer (13 tests)

134-byte compact calldata construction, validation, and L2 sequencer reorg protection.

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
| `test_sequencer_reorg_handling` | simulator | Stale state (>5s) → opportunity rejected |
| `test_sequencer_reorg_phantom_opportunity` | simulator | Zero sqrt_price (uninitialized) → rejected |
| `test_sequencer_full_outage_both_pools_stale` | simulator | Both pools stale → no false positive |
| `test_fresh_state_passes_validation` | simulator | Fresh state → validation passes (positive control) |
| `test_sequencer_reorg_abnormal_price` | simulator | Price >100K → anomaly detection |

#### Gas Spike Resilience (3 tests)

| Test | Module | Purpose |
|------|--------|---------|
| `test_circuit_breaker_on_gas_spike` | strategy | 500K Gwei spike → opportunity rejected |
| `test_gas_spike_large_spread_still_profitable` | strategy | 2% spread survives 500 Gwei spike |
| `test_zero_base_fee_uses_config_fallback` | strategy | base_fee=0 → config fallback |

#### RPC Connection Failover (5 tests)

| Test | Module | Purpose |
|------|--------|---------|
| `test_rpc_failover_without_panic` | state_sync | RPC drop → state preserved, no panic |
| `test_rpc_consecutive_failures_staleness_protection` | state_sync | Old state marked stale |
| `test_rpc_never_connected_no_panic` | state_sync | Default state (never synced) is safe |
| `test_rpc_failover_concurrent_access_no_panic` | state_sync | Multi-reader RwLock under failure |
| `test_reconnect_exponential_backoff_calculation` | state_sync | Backoff: 100ms → ... → 10s cap |

#### Key Manager (6 tests)

| Test | Module | Purpose |
|------|--------|---------|
| `test_encrypt_decrypt_roundtrip` | key_manager | Encrypt → decrypt → same key |
| `test_wrong_password_fails` | key_manager | Wrong password → decryption fails |
| `test_different_keys_produce_different_ciphertexts` | key_manager | No ciphertext collision |
| `test_corrupted_file_fails` | key_manager | Tampered file → error |
| `test_empty_key_manager` | key_manager | Empty state is safe |
| `test_env_var_fallback` | key_manager | ENV variable fallback path |

#### Route Engine (5 tests)

| Test | Module | Purpose |
|------|--------|---------|
| `test_graph_build_and_edges` | route_engine | Liquidity graph construction |
| `test_find_two_hop_routes` | route_engine | 2-hop route discovery |
| `test_find_triangular_routes` | route_engine | Triangular arbitrage routes |
| `test_max_routes_limit` | route_engine | Route count limiting |
| `test_no_self_loops` | route_engine | Self-loop prevention |

---

### Solidity Tests (69)

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

#### Miscellaneous (4+ tests)

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
| `test_JITAttack_AttackerCannotCallFallback` | Attacker ≠ executor → `Unauthorized()` |

---

## Chaos Injector — End-to-End Hell Simulation

**Script:** `Bot/chaos_injector.sh` — Live-fire adversarial stress testing against an Anvil fork.

### What It Does

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

### Running

```bash
# Full automated pipeline: Foundry fuzz → Rust tests → Anvil chaos
./run_all_tests.sh
```

---

## Dust Sweeper — Auto Token Cleanup

The Dust Sweeper scans the wallet for small ERC-20 balances (dust) and swaps them to WETH via Uniswap V3 SwapRouter02 on Base.

### Features

- Scans 7 whitelisted tokens (USDC, USDbC, DAI, cbETH, cbBTC, AERO, DEGEN)
- Per-token optimal fee tier selection (0.05% for stablecoins, 0.3% for others)
- Minimum $0.50 USD threshold (skips dust below gas cost)
- 5% slippage protection on all swaps
- MEV-protected via Private RPC (required for `--execute`)
- Dry-run by default — no transactions without `--execute` flag

### Usage

```bash
# Scan only (dry-run) — shows balances and what would be swept
cargo run -- --sweep-dust

# Execute real swaps — sends actual transactions
cargo run -- --sweep-dust --execute
```

### Supported Tokens

| Token | Fee Tier | Approx USD |
|-------|----------|------------|
| USDC | 0.05% | $1.00 |
| USDbC | 0.05% | $1.00 |
| DAI | 0.05% | $1.00 |
| cbETH | 0.05% | ~ETH |
| cbBTC | 0.3% | ~BTC |
| AERO | 0.3% | ~$1.50 |
| DEGEN | 0.3% | ~$0.01 |

---

## Structured JSON Logging

All bot events are logged to structured JSONL files for analysis and monitoring.

### Log Location

```
Bot/logs/bot_events.jsonl
```

### Features

- One JSON object per line (JSONL format)
- ISO 8601 UTC timestamps
- Auto-rotation at 50MB (renamed to `.jsonl.1`, `.jsonl.2`, etc.)
- Thread-safe file writes

### Log Functions

| Function | When Used |
|----------|-----------|
| `log_json(level, event, data)` | General purpose logging |
| `log_opportunity(data)` | Arbitrage opportunity detected |
| `log_trade(data)` | Successful trade execution |
| `log_error(data)` | Error conditions |
| `log_discovery(data)` | New pool discovered |

### Example Entry

```json
{"timestamp":"2026-03-14T10:30:15.123Z","level":"trade","event":"dust_sweep","data":{"token":"USDC","amount":1.5,"value_usd":1.5,"tx_hash":"0xabc..."}}
```

---

## Project Structure

```
god_tier_arbitraj/
├── README.md                    ← You are here
├── CHANGELOG_v25_session.md     ← Detailed session changelog
├── run_all_tests.sh             ← Full test pipeline script
│
├── Bot/                         ← Rust off-chain arbitrage bot
│   ├── Cargo.toml               ← Dependencies + release profile
│   ├── .gitignore               ← Excludes .env, .keystore, target/
│   ├── .env                     ← Configuration (git-ignored)
│   ├── chaos_injector.sh        ← Chaos Injection stress test script
│   ├── logs/                    ← JSONL structured log output
│   │   └── bot_events.jsonl
│   └── src/
│       ├── main.rs              ← Entry point, block loop, CLI, hot-reload
│       ├── types.rs             ← BotConfig, PoolState, SharedPoolState
│       ├── math.rs              ← AMM math: multi-tick, Newton-Raphson, U256
│       ├── state_sync.rs        ← Multicall3 sync, TickBitmap management
│       ├── simulator.rs         ← REVM simulation, calldata encoding
│       ├── strategy.rs          ← Opportunity detection, TX building
│       ├── key_manager.rs       ← AES-256-GCM encrypted key management
│       ├── discovery_engine.rs  ← Factory events, V2 filtering, API discovery
│       ├── pool_discovery.rs    ← DexScreener API, pair matching
│       ├── transport.rs         ← HFT RPC pool, health monitoring
│       ├── executor.rs          ← Private RPC MEV protection
│       ├── route_engine.rs      ← Liquidity graph, multi-hop DFS
│       ├── json_logger.rs       ← Structured JSONL logging
│       └── dust_sweeper.rs      ← ERC-20 dust auto-sweep to WETH
│
├── Contract/                    ← Solidity smart contract (Foundry)
│   ├── foundry.toml             ← Foundry config (Cancun EVM, optimizer 1M runs)
│   ├── src/
│   │   └── Arbitraj.sol         ← ArbitrajBotu v25.0 (599 lines, 134B calldata)
│   ├── test/
│   │   └── Arbitraj.t.sol       ← 69 tests (unit + fuzz + adversarial)
│   └── lib/
│       ├── forge-std/           ← Foundry test framework
│       └── aave-v3-core/        ← AAVE V3 (flash loan reference)
```

---

## Quick Start

### Prerequisites

- **Rust** >= 1.75 (with `cargo`)
- **Foundry** (`forge`, `cast`, `anvil`)
- A Base L2 RPC endpoint (dRPC, Alchemy, Infura, etc.)

### Build & Test

```bash
# 1. Compile and test the Rust bot
cd Bot
cargo test              # 45 tests

# 2. Compile and test the Solidity contract
cd Contract
forge test              # 69 tests

# 3. Run the full pipeline (fuzz + unit + chaos)
./run_all_tests.sh

# 4. Build for production (optimized)
cd Bot
cargo build --release   # LTO + opt-level 3
```

### Configuration

Create `Bot/.env` (see `.env.example` for template):

```env
# RPC Connections
RPC_WSS_URL=wss://base-mainnet.your-rpc.io
RPC_HTTP_URL=https://base-mainnet.your-rpc.io

# Contract
ARBITRAGE_CONTRACT_ADDRESS=0x...

# MEV Protection (Base L2)
PRIVATE_RPC_URL=https://rpc.flashbots.net?chainId=8453

# Keys — use encrypted keystore, NEVER put keys in .env
KEYSTORE_PATH=./keystore.json

# Strategy
MIN_NET_PROFIT_WETH=0.00003     # ~$0.075 (Base L2 micro-profit)
GAS_COST_FALLBACK_WETH=0.00005
MAX_TRADE_SIZE_WETH=50.0
TICK_BITMAP_RANGE=100            # Word range for bitmap sync
EXECUTION_ENABLED=false          # Set true for live execution
```

### Key Management

```bash
# Create encrypted keystore (recommended)
cargo run -- --encrypt-key

# Or set PRIVATE_KEY in .env (not recommended for production)
```

---

## CLI Reference

| Command | Description |
|---------|-------------|
| `cargo run` | Start bot in shadow mode (default) |
| `cargo run --release` | Start bot with optimized binary |
| `cargo run -- --encrypt-key` | Create AES-256-GCM encrypted keystore |
| `cargo run -- --sweep-dust` | Scan wallet for dust tokens (dry-run) |
| `cargo run -- --sweep-dust --execute` | Sweep dust tokens to WETH (real TXs) |

---

## Tech Stack

| Layer | Technology | Purpose |
|-------|------------|---------|
| Smart Contract | Solidity 0.8.27, Cancun EVM | On-chain arbitrage execution |
| Bot Runtime | Rust, Tokio | Async block processing |
| EVM Interface | Alloy 1.7 | RPC calls, TX signing, types |
| Local Simulation | REVM 36 | Zero-latency EVM execution |
| State Locking | parking_lot RwLock | Lock-free concurrent reads |
| Cryptography | AES-256-GCM, PBKDF2 | Key encryption |
| MEV Protection | Flashbots Protect (Base) | Private TX submission |
| Pool Discovery | DexScreener API + Factory Events | Auto pair detection |
| Logging | Custom JSONL + colored terminal | Structured analysis |
| Testing | Foundry (Forge), cargo test, proptest | Fuzz + unit + chaos |
| Release Profile | LTO fat, codegen-units=1 | Max performance binary |

---

<p align="center"><b>Built for Base L2 — Optimized for speed, hardened by chaos.</b></p>
