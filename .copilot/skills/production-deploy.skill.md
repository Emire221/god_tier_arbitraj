---
name: "Production Deploy"
description: "Production deployment pipeline — Bot binary ve Contract deployment"
---

# 🚀 PRODUCTION DEPLOY SKILL

> **Amaç:** Güvenli production deployment — Bot release build, Contract mainnet deploy, health checks.

## KULLANIM SENARYOLARI

### Senaryo 1: Full Production Deploy
```
"Production'a deploy et"
"Mainnet'e çıkalım"
```

### Senaryo 2: Hot Reload (Bot Only)
```
"Bot'u güncelle, kontrat aynı kalsın"
"Yeni binary deploy et"
```

### Senaryo 3: Contract Upgrade
```
"Kontratı yeni versiyona geçir"
"Admin fonksiyonlarını güncelle"
```

## PRE-DEPLOY CHECKLIST

```
═══════════════════════════════════════════════════════════════
📋 PRE-DEPLOY CHECKLIST
═══════════════════════════════════════════════════════════════

🦀 RUST BOT:
├── [ ] cargo check                    ✅
├── [ ] cargo clippy -- -D warnings    ✅ 0 warnings
├── [ ] cargo test --release           ✅ All passed
├── [ ] cargo build --release          ✅ Binary built
├── [ ] .env validation                ✅ All keys present
├── [ ] PRIVATE_RPC_URL set            ✅ Private endpoint
└── [ ] Wallet balance check           ✅ Sufficient ETH/gas

⛽ SOLIDITY CONTRACT:
├── [ ] forge build                    ✅ No errors
├── [ ] forge test -vvv                ✅ All passed
├── [ ] forge test --gas-report        ✅ Within limits
├── [ ] forge test --fork-url          ✅ Fork tests pass
├── [ ] Security audit                 ✅ No vulnerabilities
└── [ ] Admin/Executor keys            ✅ Configured

🔐 SECURITY:
├── [ ] Private keys encrypted         ✅ AES-256-GCM
├── [ ] .env not in git                ✅ .gitignore
├── [ ] Admin key offline              ✅ Cold wallet
├── [ ] Executor key minimal balance   ✅ Only for gas
└── [ ] Pool whitelist configured      ✅ Only trusted pools

📊 INFRASTRUCTURE:
├── [ ] Private RPC endpoint           ✅ Low latency
├── [ ] Backup RPC configured          ✅ Failover ready
├── [ ] Telegram alerts                ✅ Notifications active
└── [ ] Monitoring dashboard           ✅ Metrics visible

═══════════════════════════════════════════════════════════════
```

## DEPLOY PIPELINE

```
┌─────────────────────────────────────────────────────────────┐
│                   PRODUCTION DEPLOY PIPELINE                │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  STAGE 1: VALIDATION                                        │
│  ├── Full system test                                       │
│  ├── Security scan                                          │
│  └── .env validation                                        │
│                                                             │
│  STAGE 2: BUILD                                             │
│  ├── cargo build --release (Bot)                            │
│  └── forge build (Contract)                                 │
│                                                             │
│  STAGE 3: DRY RUN (Anvil Fork)                              │
│  ├── Local fork deployment                                  │
│  ├── Simulated arbitrage                                    │
│  └── Gas estimation                                         │
│                                                             │
│  STAGE 4: CONTRACT DEPLOY (if needed)                       │
│  ├── Verify constructor args                                │
│  ├── Deploy to Base mainnet                                 │
│  ├── Verify on Basescan                                     │
│  └── Pool whitelist initialization                          │
│                                                             │
│  STAGE 5: BOT DEPLOY                                        │
│  ├── Stop current instance                                  │
│  ├── Backup current binary                                  │
│  ├── Deploy new binary                                      │
│  └── Start with health check                                │
│                                                             │
│  STAGE 6: POST-DEPLOY VALIDATION                            │
│  ├── Shadow mode test                                       │
│  ├── First arbitrage (if opportunity)                       │
│  └── Monitoring verification                                │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

## STAGE DETAYLARI

### Stage 1: Validation

```bash
# Full System Test (skill: full-system-test)
cd Bot && cargo test --release
cd ../Contract && forge test -vvv

# Security Scan
cargo audit  # Rust dependencies
forge audit  # Solidity (if available)

# .env Validation
required_vars=(
    "CHAIN_RPC_URL"
    "CHAIN_WSS_URL"
    "PRIVATE_RPC_URL"
    "CONTRACT_ADDRESS"
    "EXECUTOR_PRIVATE_KEY"
    "TELEGRAM_BOT_TOKEN"
    "TELEGRAM_CHAT_ID"
)

for var in "${required_vars[@]}"; do
    if [ -z "${!var}" ]; then
        echo "ERROR: $var not set"
        exit 1
    fi
done
```

### Stage 2: Build

```bash
# Bot Build
cd Bot
cargo build --release --target x86_64-unknown-linux-gnu
# Output: target/release/arbitraj_botu

# Binary info
ls -la target/release/arbitraj_botu
# Expected: ~15-25 MB, optimized

# Contract Build
cd ../Contract
forge build
# Output: out/Arbitraj.sol/Arbitraj.json
```

### Stage 3: Dry Run (Anvil)

```bash
# Start Anvil fork
anvil --fork-url $BASE_RPC_URL --fork-block-number $LATEST_BLOCK

# Deploy contract to fork
forge script script/Deploy.s.sol:DeployScript \
    --rpc-url http://localhost:8545 \
    --broadcast

# Simulate arbitrage
forge test --match-test test_RealArbitrage \
    --fork-url http://localhost:8545 \
    -vvvv
```

### Stage 4: Contract Deploy

```bash
# Mainnet Deployment
cd Contract

# Deploy (requires admin key)
forge script script/Deploy.s.sol:DeployScript \
    --rpc-url $BASE_RPC_URL \
    --private-key $ADMIN_PRIVATE_KEY \
    --broadcast \
    --verify \
    --etherscan-api-key $BASESCAN_API_KEY

# Output:
# Contract deployed at: 0x...
# Transaction hash: 0x...

# Initialize pool whitelist
cast send $CONTRACT_ADDRESS \
    "executorBatchAddPools(address[])" \
    "[0x..., 0x..., 0x...]" \
    --rpc-url $BASE_RPC_URL \
    --private-key $EXECUTOR_PRIVATE_KEY
```

### Stage 5: Bot Deploy

```bash
# SSH to production server (example)
ssh prod-server

# Stop current instance
systemctl stop arbitraj-bot

# Backup
cp /opt/arbitraj/arbitraj_botu /opt/arbitraj/arbitraj_botu.bak

# Deploy new binary
scp target/release/arbitraj_botu prod-server:/opt/arbitraj/

# Start
systemctl start arbitraj-bot

# Health check
curl http://localhost:8080/health
```

### Stage 6: Post-Deploy Validation

```bash
# Check logs
journalctl -u arbitraj-bot -f

# Shadow mode verification
# Look for:
# ✅ [v25.0] Pool sync: 3.2ms
# ✅ [v25.0] Simulation: 45ms
# ✅ [v25.0] Opportunity found: +0.005 ETH

# Telegram notification test
curl -X POST https://api.telegram.org/bot$TOKEN/sendMessage \
    -d "chat_id=$CHAT_ID" \
    -d "text=🤖 Bot v25.0 deployed successfully!"
```

## ROLLBACK PROCEDURE

```bash
# Eğer deployment başarısız olursa:

# 1. Bot rollback
systemctl stop arbitraj-bot
cp /opt/arbitraj/arbitraj_botu.bak /opt/arbitraj/arbitraj_botu
systemctl start arbitraj-bot

# 2. Contract rollback (immutable, yeni deploy gerekli)
# Admin key ile yeni kontrat deploy et
# Bot .env'de CONTRACT_ADDRESS güncelle
# Pool whitelist'i yeniden initialize et
```

## .ENV TEMPLATE

```bash
# ═══════════════════════════════════════════════════════════
# GOD TIER ARBITRAJ — PRODUCTION .env
# ═══════════════════════════════════════════════════════════

# ── RPC Endpoints ──
CHAIN_RPC_URL=https://base-mainnet.g.alchemy.com/v2/YOUR_KEY
CHAIN_WSS_URL=wss://base-mainnet.g.alchemy.com/v2/YOUR_KEY
PRIVATE_RPC_URL=https://rpc.flashbots.net  # MEV protection

# ── Contract ──
CONTRACT_ADDRESS=0x...  # Deployed contract
WETH_ADDRESS=0x4200000000000000000000000000000000000006  # Base WETH

# ── Keys (ENCRYPTED recommended) ──
EXECUTOR_PRIVATE_KEY=0x...  # Hot wallet
# ADMIN_PRIVATE_KEY=...     # NEVER in .env, use hardware wallet

# ── Notifications ──
TELEGRAM_BOT_TOKEN=...
TELEGRAM_CHAT_ID=...

# ── Thresholds ──
MIN_PROFIT_USD=1.00
MAX_STALENESS_MS=1000
SHADOW_MODE=false

# ═══════════════════════════════════════════════════════════
```

## ÇIKIŞ RAPORU

```
═══════════════════════════════════════════════════════════════
🚀 PRODUCTION DEPLOY REPORT
═══════════════════════════════════════════════════════════════
Zaman: [timestamp]
Versiyon: v25.0.0
Durum: ✅ BAŞARILI | ❌ BAŞARISIZ | 🔄 ROLLBACK

📦 DEPLOYMENT DETAYLARI:

┌─────────────────────────────────────────────────────────────┐
│ COMPONENT          │ STATUS     │ DETAILS                  │
├────────────────────┼────────────┼──────────────────────────┤
│ Bot Binary         │ ✅ DEPLOY  │ 18.3 MB, x86_64-linux    │
│ Contract           │ ⏸️ SKIP    │ Already deployed         │
│ Pool Whitelist     │ ✅ UPDATED │ +5 new pools             │
│ .env Config        │ ✅ VALID   │ All vars present         │
│ Health Check       │ ✅ PASS    │ Response: 200 OK         │
└─────────────────────────────────────────────────────────────┘

🔐 SECURITY VERIFICATION:
├── Private RPC:     ✅ Flashbots endpoint
├── Admin key:       ✅ Offline (cold wallet)
├── Executor key:    ✅ Minimal balance (0.05 ETH)
└── Telegram:        ✅ Alerts configured

📊 POST-DEPLOY METRICS:
├── First sync:      3.1ms ✅
├── Simulation:      42ms ✅
├── Memory:          45 MB ✅
└── CPU:             2% idle ✅

📝 NOTES:
- Shadow mode disabled, live trading active
- 12 pools in whitelist
- Telegram notification sent

═══════════════════════════════════════════════════════════════
```

## EMERGENCY PROCEDURES

### Bot Emergency Stop
```bash
systemctl stop arbitraj-bot
# veya
kill -9 $(pgrep arbitraj_botu)
```

### Contract Emergency (Admin Only)
```solidity
// Tüm fonları admin'e çek
withdrawToken(WETH, type(uint256).max);
withdrawETH();
```

### Post-Incident
```
1. Root cause analysis
2. Fix development (self-healing skill)
3. Full system test
4. Staged rollout (shadow → live)
```

---

*"Deploy with confidence, monitor relentlessly."*
