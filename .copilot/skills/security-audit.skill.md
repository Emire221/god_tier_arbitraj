---
name: "Security Audit"
description: "Kapsamlı güvenlik denetimi — MEV, reentrancy, access control, key management"
---

# 🛡️ SECURITY AUDIT SKILL

> **Amaç:** Bot ve Contract için kapsamlı güvenlik denetimi — MEV koruması, reentrancy, access control, key management.

## KULLANIM SENARYOLARI

### Senaryo 1: Full Security Audit
```
"Güvenlik denetimi yap"
"Tüm sistemi güvenlik açısından tara"
```

### Senaryo 2: Spesifik Audit
```
"Callback güvenliğini kontrol et"
"MEV saldırı vektörlerini analiz et"
```

### Senaryo 3: Pre-Deploy Security
```
"Production öncesi güvenlik kontrolü"
"Kontratı audit et"
```

## SALDIRI VEKTÖRLERİ MATRİSİ

```
┌─────────────────────────────────────────────────────────────┐
│                    ATTACK VECTOR MATRIX                     │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  VECTOR              │ SEVERITY │ STATUS  │ MITIGATION      │
│  ────────────────────┼──────────┼─────────┼────────────────  │
│  Sandwich Attack     │ HIGH     │ ✅ SAFE │ minProfit       │
│  Frontrunning        │ HIGH     │ ✅ SAFE │ Private RPC     │
│  Reentrancy          │ CRITICAL │ ✅ SAFE │ tstore lock     │
│  Callback Spoofing   │ CRITICAL │ ✅ SAFE │ msg.sender check│
│  Flash Loan Attack   │ HIGH     │ ✅ SAFE │ Profit check    │
│  Price Manipulation  │ HIGH     │ ✅ SAFE │ Multi-source    │
│  Stale Data Attack   │ MEDIUM   │ ✅ SAFE │ Staleness check │
│  Gas Griefing        │ MEDIUM   │ ✅ SAFE │ Gas limit       │
│  Key Compromise      │ CRITICAL │ ✅ SAFE │ Encrypted keys  │
│  Admin Key Theft     │ CRITICAL │ ✅ SAFE │ Cold wallet     │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

## AUDIT CHECKLIST

### 1. Smart Contract Security

#### Reentrancy Guard
```solidity
// ✅ DOĞRU: EIP-1153 transient storage lock
uint256 locked;
assembly { locked := tload(0xFF) }
if (locked != 0) revert Locked();
assembly { tstore(0xFF, 1) }
// ... external calls ...
assembly { tstore(0xFF, 0) }

// ❌ VULNERABLE: No lock or improper CEI
function withdraw() external {
    token.transfer(msg.sender, amount); // External call FIRST
    balance = 0;                        // State change AFTER
}
```

#### Callback Validation
```solidity
// ✅ DOĞRU: Strict msg.sender validation
function uniswapV3SwapCallback(...) external {
    address expected;
    assembly { expected := tload(0x00) }
    if (msg.sender != expected) revert InvalidCaller();
}

// ❌ VULNERABLE: No validation
function uniswapV3SwapCallback(...) external {
    // Anyone can call this!
}
```

#### Access Control
```solidity
// ✅ DOĞRU: Role separation
address public immutable executor; // Hot wallet - arbitrage only
address public immutable admin;    // Cold wallet - fund management

fallback() external {
    if (msg.sender != executor) revert Unauthorized();
}

function withdrawToken(...) external {
    if (msg.sender != admin) revert Unauthorized();
}

// ❌ VULNERABLE: Single owner
address public owner;
function anyFunction() external {
    require(msg.sender == owner); // All eggs in one basket
}
```

#### Overflow Protection
```solidity
// ✅ DOĞRU: Check before unchecked
if (balAfter <= balBefore) revert NoProfitRealized();
unchecked {
    profit = balAfter - balBefore; // Safe: checked above
}

// ❌ VULNERABLE: Blind unchecked
unchecked {
    profit = balAfter - balBefore; // Underflow possible!
}
```

### 2. MEV Protection

#### Sandwich Protection
```solidity
// ✅ DOĞRU: minProfit threshold
if (profit < minProfit) revert InsufficientProfit();
// If sandwich reduces profit below threshold, TX reverts

// ❌ VULNERABLE: No profit check
// Attacker can sandwich and steal all profit
```

#### Frontrun Protection
```rust
// ✅ DOĞRU: Private RPC
let provider = ProviderBuilder::new()
    .connect_http(env::var("PRIVATE_RPC_URL")?);
// TX not visible in public mempool

// ❌ VULNERABLE: Public RPC
let provider = ProviderBuilder::new()
    .connect_http("https://base-mainnet.public.blastapi.io");
// TX visible, can be frontrun
```

#### Deadline Protection
```solidity
// ✅ DOĞRU: Block deadline
uint32 deadline;
assembly { deadline := shr(224, calldataload(0x82)) }
if (block.number > deadline) revert DeadlineExpired();
// Stale TX in mempool will revert

// ❌ VULNERABLE: No deadline
// Old TX can execute at unfavorable price
```

### 3. Key Management

#### Encrypted Storage
```rust
// ✅ DOĞRU: AES-256-GCM encryption
let encrypted_key = aes_gcm::encrypt(
    &key_encryption_key,
    &nonce,
    private_key_bytes,
)?;
// Key at rest is encrypted

// ❌ VULNERABLE: Plain text
let private_key = std::fs::read_to_string(".env")?;
// Key visible in file system
```

#### Role Separation
```
✅ DOĞRU:
├── Executor (Hot Wallet)
│   ├── Minimal balance (~0.05 ETH for gas)
│   ├── Can ONLY call fallback() for arbitrage
│   └── Compromise = limited loss
│
└── Admin (Cold Wallet)
    ├── Offline storage (hardware wallet)
    ├── Can withdraw funds
    └── Never online during operation
```

### 4. Rust Bot Security

#### No Panics in Production
```rust
// ✅ DOĞRU: Graceful error handling
let value = result.map_err(|e| eyre::eyre!("Context: {}", e))?;

// ❌ VULNERABLE: Panic stops bot
let value = result.unwrap(); // PANIC if Err
```

#### Input Validation
```rust
// ✅ DOĞRU: Validate all inputs
fn process_pool(address: Address) -> Result<()> {
    if address.is_zero() {
        return Err(eyre::eyre!("Invalid zero address"));
    }
    // ...
}

// ❌ VULNERABLE: No validation
fn process_pool(address: Address) -> Result<()> {
    // Assume address is valid
}
```

#### Secret Handling
```rust
// ✅ DOĞRU: Zeroize secrets
use zeroize::Zeroize;

struct PrivateKey(#[zeroize(drop)] [u8; 32]);

// ❌ VULNERABLE: Secrets in memory
let private_key: [u8; 32] = load_key();
// Key remains in memory after use
```

## AUTOMATİK SECURITY SCAN

```bash
# Rust security audit
cargo audit

# Clippy security lints
cargo clippy -- \
    -W clippy::mem_forget \
    -W clippy::missing_panics_doc \
    -W clippy::unwrap_used \
    -W clippy::expect_used

# Solidity static analysis
slither Contract/src/Arbitraj.sol

# Foundry invariant tests
forge test --match-contract Invariant
```

## SECURITY TEST CASES

### Contract Tests
```solidity
// test/Security.t.sol

function test_RevertWhen_ReentrancyAttempt() public {
    ReentrantAttacker attacker = new ReentrantAttacker(address(executor));
    vm.expectRevert(ArbitrageExecutor.Locked.selector);
    attacker.attack();
}

function test_RevertWhen_CallbackFromWrongAddress() public {
    vm.prank(address(0xDEAD));
    vm.expectRevert(ArbitrageExecutor.InvalidCaller.selector);
    executor.uniswapV3SwapCallback(0, 0, "");
}

function test_RevertWhen_UnauthorizedWithdraw() public {
    vm.prank(address(0xDEAD));
    vm.expectRevert(ArbitrageExecutor.Unauthorized.selector);
    executor.withdrawToken(address(weth), 1 ether);
}

function testFuzz_NoOverflowOnAnyInput(uint256 amount) public {
    // Fuzz test for overflow conditions
}
```

### Bot Tests
```rust
#[test]
fn test_rejects_zero_address() {
    let result = process_pool(Address::ZERO);
    assert!(result.is_err());
}

#[test]
fn test_private_key_zeroized() {
    let key = PrivateKey::load().unwrap();
    drop(key);
    // Memory should be zeroed
}
```

## ÇIKIŞ RAPORU

```
═══════════════════════════════════════════════════════════════
🛡️ SECURITY AUDIT REPORT
═══════════════════════════════════════════════════════════════
Tarih: [timestamp]
Versiyon: v25.0
Auditor: Copilot Security Scan
Genel Durum: ✅ GÜVENLİ | ⚠️ UYARILAR VAR | ❌ KRİTİK AÇIK

📋 AUDIT SONUÇLARI:

┌─────────────────────────────────────────────────────────────┐
│ CATEGORY            │ CHECKS │ PASSED │ STATUS             │
├─────────────────────┼────────┼────────┼────────────────────┤
│ Reentrancy          │ 3      │ 3      │ ✅ SECURE          │
│ Access Control      │ 5      │ 5      │ ✅ SECURE          │
│ Callback Validation │ 2      │ 2      │ ✅ SECURE          │
│ MEV Protection      │ 4      │ 4      │ ✅ SECURE          │
│ Key Management      │ 3      │ 3      │ ✅ SECURE          │
│ Input Validation    │ 6      │ 6      │ ✅ SECURE          │
│ Overflow Protection │ 4      │ 4      │ ✅ SECURE          │
├─────────────────────┼────────┼────────┼────────────────────┤
│ TOTAL               │ 27     │ 27     │ ✅ ALL PASSED      │
└─────────────────────────────────────────────────────────────┘

🔍 DETAYLI BULGULAR:

✅ Reentrancy Guard: EIP-1153 transient storage lock aktif
✅ Callback Auth: msg.sender doğrulaması mevcut
✅ Access Control: Executor/Admin ayrımı uygulanmış
✅ MEV Protection: minProfit + Private RPC + Deadline
✅ Key Management: AES-256-GCM encryption + Cold wallet

⚠️ ÖNERİLER:
├── Consider adding rate limiting for executor
├── Add monitoring for unusual transaction patterns
└── Regular key rotation for executor wallet

📊 AUTOMATİK SCAN SONUÇLARI:
├── cargo audit:    ✅ 0 vulnerabilities
├── clippy security: ✅ 0 warnings
├── slither:        ✅ No high/medium findings
└── invariant tests: ✅ 10000 runs passed

═══════════════════════════════════════════════════════════════
```

## INCIDENT RESPONSE PLAN

```
┌─────────────────────────────────────────────────────────────┐
│                 INCIDENT RESPONSE PLAN                      │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  1. DETECTION                                               │
│     ├── Telegram alerts: unusual activity                   │
│     ├── Balance monitoring: unexpected changes              │
│     └── Failed TX spike: potential attack                   │
│                                                             │
│  2. CONTAINMENT (< 5 min)                                   │
│     ├── Stop bot immediately                                │
│     ├── Revoke executor key (if compromised)                │
│     └── Withdraw funds via admin key                        │
│                                                             │
│  3. INVESTIGATION                                           │
│     ├── Analyze transaction history                         │
│     ├── Check for unauthorized calls                        │
│     └── Identify attack vector                              │
│                                                             │
│  4. RECOVERY                                                │
│     ├── Deploy patched contract (if needed)                 │
│     ├── Generate new executor key                           │
│     ├── Update pool whitelist                               │
│     └── Resume with shadow mode                             │
│                                                             │
│  5. POST-MORTEM                                             │
│     ├── Document timeline                                   │
│     ├── Root cause analysis                                 │
│     └── Update security measures                            │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

---

*"Güvenlik, en yüksek kârlılıktır."*
