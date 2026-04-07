---
name: "Emergency Response"
description: "Acil durum müdahalesi — kritik hata, güvenlik açığı, sistem çökmesi"
---

# 🚨 EMERGENCY RESPONSE SKILL

> **Amaç:** Kritik sistem hatalarına anında müdahale — bot durdurma, fon güvenliği, kök neden analizi.

## ACİL DURUM TİPLERİ

| Seviye | Tip | Örnek | Müdahale Süresi |
|--------|-----|-------|-----------------|
| 🔴 P0 | Güvenlik İhlali | Key compromise, exploit | < 5 dakika |
| 🔴 P0 | Fon Kaybı | Unexpected withdrawal | < 5 dakika |
| 🟠 P1 | Sistem Çökmesi | Bot panic, contract revert | < 15 dakika |
| 🟠 P1 | Performans Çöküşü | Latency > 500ms | < 30 dakika |
| 🟡 P2 | Degraded Service | High revert rate | < 2 saat |

## ACİL DURDURMA PROSEDÜRÜ

### Bot Emergency Stop

```bash
# Yöntem 1: Systemd
sudo systemctl stop arbitraj-bot

# Yöntem 2: Process kill
kill -9 $(pgrep arbitraj_botu)

# Yöntem 3: Remote (SSH)
ssh prod-server "systemctl stop arbitraj-bot"

# Doğrulama
pgrep arbitraj_botu || echo "Bot stopped successfully"
```

### Contract Emergency (Admin Key Required)

```bash
# Tüm WETH'i çek
cast send $CONTRACT_ADDRESS \
    "withdrawToken(address,uint256)" \
    $WETH_ADDRESS \
    $(cast call $CONTRACT_ADDRESS "balanceOf(address)" $CONTRACT_ADDRESS) \
    --rpc-url $BASE_RPC_URL \
    --private-key $ADMIN_PRIVATE_KEY

# Tüm ETH'i çek
cast send $CONTRACT_ADDRESS \
    "withdrawETH()" \
    --rpc-url $BASE_RPC_URL \
    --private-key $ADMIN_PRIVATE_KEY
```

## OLAY MÜDAHALE AŞAMALARI

```
┌─────────────────────────────────────────────────────────────┐
│               INCIDENT RESPONSE PHASES                      │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  PHASE 1: DETECTION (T+0)                                   │
│  ├── Alert received (Telegram/monitoring)                   │
│  ├── Severity assessment                                    │
│  └── Escalation decision                                    │
│                                                             │
│  PHASE 2: CONTAINMENT (T+5min)                              │
│  ├── Stop bot                                               │
│  ├── Secure funds (if P0)                                   │
│  └── Preserve evidence (logs, state)                        │
│                                                             │
│  PHASE 3: INVESTIGATION (T+15min)                           │
│  ├── Log analysis                                           │
│  ├── Transaction trace                                      │
│  └── Root cause identification                              │
│                                                             │
│  PHASE 4: REMEDIATION (T+1h)                                │
│  ├── Develop fix                                            │
│  ├── Test fix                                               │
│  └── Prepare deployment                                     │
│                                                             │
│  PHASE 5: RECOVERY (T+2h)                                   │
│  ├── Deploy fix (shadow mode first)                         │
│  ├── Gradual rollout                                        │
│  └── Monitor closely                                        │
│                                                             │
│  PHASE 6: POST-MORTEM (T+24h)                               │
│  ├── Document timeline                                      │
│  ├── Lessons learned                                        │
│  └── Process improvements                                   │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

## SENARYO PLAYBOOK'LARI

### Senaryo 1: Bot Panic (P1)

```
SYMPTOM: Bot process crashed with panic
IMPACT: No arbitrage execution
URGENCY: Medium (no fund risk)

STEPS:
1. Check logs for panic message
   journalctl -u arbitraj-bot --since "10 minutes ago"

2. Identify panic source
   grep "panicked at" /var/log/arbitraj/bot.log

3. Check if unwrap()/expect() violation
   → If yes: @rust-ninja fix required

4. Attempt restart
   systemctl restart arbitraj-bot

5. If panic persists: rollback to previous version
   cp /opt/arbitraj/arbitraj_botu.bak /opt/arbitraj/arbitraj_botu
   systemctl restart arbitraj-bot

6. Root cause fix (after stabilization)
```

### Senaryo 2: Contract Revert Spike (P1)

```
SYMPTOM: > 50% of TXs reverting
IMPACT: Lost gas, no profit
URGENCY: High

STEPS:
1. Analyze revert reasons
   cast run $TX_HASH --rpc-url $BASE_RPC_URL

2. Common causes:
   ├── InsufficientProfit: Pool state changed
   ├── DeadlineExpired: TX stuck in mempool
   ├── Locked: Reentrancy guard (shouldn't happen)
   └── InvalidCaller: Callback issue

3. Check pool states
   cast call $POOL "slot0()" --rpc-url $BASE_RPC_URL

4. If pool state stale:
   → Check RPC connectivity
   → Verify sync latency

5. If systematic issue:
   → Stop bot
   → Investigate root cause
```

### Senaryo 3: Key Compromise (P0)

```
SYMPTOM: Unauthorized transaction from executor wallet
IMPACT: CRITICAL - potential fund loss
URGENCY: IMMEDIATE

STEPS:
1. IMMEDIATELY stop bot
   kill -9 $(pgrep arbitraj_botu)

2. IMMEDIATELY withdraw all funds via admin key
   # Use hardware wallet, NOT compromised machine
   cast send $CONTRACT "withdrawToken(address,uint256)" \
       $WETH $BALANCE --private-key $ADMIN_KEY

3. Revoke executor permissions (if possible)
   # Current contract: executor is immutable
   # → Deploy new contract required

4. Generate new executor key
   # On a CLEAN machine
   cast wallet new

5. Deploy new contract
   forge script Deploy.s.sol --private-key $ADMIN_KEY

6. Transfer funds to new contract

7. Update bot config with new contract + executor

8. Full security audit before resuming

9. Post-mortem: How was key compromised?
```

### Senaryo 4: Unexpected Loss (P0)

```
SYMPTOM: Contract balance decreased unexpectedly
IMPACT: CRITICAL - fund loss confirmed
URGENCY: IMMEDIATE

STEPS:
1. STOP EVERYTHING
   kill -9 $(pgrep arbitraj_botu)

2. Secure remaining funds
   cast send $CONTRACT "withdrawToken(...)"

3. Analyze transactions
   # Find the problematic TX
   cast tx $TX_HASH --rpc-url $BASE_RPC_URL
   cast run $TX_HASH --rpc-url $BASE_RPC_URL

4. Check for:
   ├── External exploit
   ├── Bug in our code
   ├── Unexpected callback behavior
   └── MEV attack that bypassed protection

5. If exploit found:
   → Do NOT publicize (may invite copycat)
   → Contact security researchers
   → Prepare hotfix quietly

6. Full security audit required before resuming
```

## LOG ANALİZ KOMUTLARI

```bash
# Son 100 hata
journalctl -u arbitraj-bot | grep -i "error" | tail -100

# Panic mesajları
journalctl -u arbitraj-bot | grep -i "panic"

# Revert reasons
grep "reverted" /var/log/arbitraj/shadow_analytics.jsonl | jq '.reason'

# TX failure timeline
grep "status.*failed" /var/log/arbitraj/shadow_analytics.jsonl | \
    jq -r '[.timestamp, .reason] | @tsv' | sort

# Gas spike detection
grep "gas_used" /var/log/arbitraj/shadow_analytics.jsonl | \
    jq '.gas_used' | awk '$1 > 300000 {print}'
```

## İLETİŞİM TEMPLATE'LERİ

### Telegram Alert
```
🚨 *EMERGENCY ALERT*

Type: [P0/P1/P2]
Time: [timestamp]
Status: [DETECTED/CONTAINED/RESOLVED]

*Description:*
[Brief description of the issue]

*Impact:*
[What is affected]

*Action Taken:*
[What has been done]

*Next Steps:*
[What will be done]

_Auto-generated by Arbitrage Bot Emergency System_
```

### Post-Mortem Template
```markdown
# Incident Post-Mortem

**Date:** [YYYY-MM-DD]
**Severity:** [P0/P1/P2]
**Duration:** [HH:MM]
**Impact:** [Description]

## Timeline
- T+0: [Detection]
- T+X: [Containment]
- T+Y: [Resolution]

## Root Cause
[Detailed explanation]

## Impact Analysis
- Financial: [ETH/USD lost or at risk]
- Operational: [Downtime duration]
- Reputation: [If applicable]

## Resolution
[What was done to fix]

## Lessons Learned
1. [Learning 1]
2. [Learning 2]

## Action Items
- [ ] [Preventive measure 1]
- [ ] [Preventive measure 2]
```

## ÖNCELİK MATRİSİ

```
                    IMPACT
              LOW       HIGH
         ┌─────────┬─────────┐
    LOW  │   P3    │   P2    │
URGENCY  ├─────────┼─────────┤
    HIGH │   P2    │   P0/P1 │
         └─────────┴─────────┘

P0: Immediate response (security, fund loss)
P1: Same day resolution (system down)
P2: Next business day (degraded service)
P3: Backlog (minor issues)
```

## ÇIKIŞ RAPORU

```
═══════════════════════════════════════════════════════════════
🚨 EMERGENCY RESPONSE REPORT
═══════════════════════════════════════════════════════════════
Incident ID: INC-2026-04-01-001
Severity: [P0|P1|P2]
Status: ✅ RESOLVED | 🔄 IN PROGRESS | ❌ ESCALATED

📋 INCIDENT SUMMARY:
[Brief description]

⏱️ TIMELINE:
├── Detection:    [timestamp]
├── Containment:  [timestamp]
├── Resolution:   [timestamp]
└── Total Time:   [duration]

💰 IMPACT:
├── Funds at Risk: X.XX ETH
├── Funds Lost:    0.00 ETH
├── Downtime:      XX minutes
└── Missed Arbs:   ~Y opportunities

🔧 RESOLUTION:
[What was done]

✅ VERIFICATION:
├── Bot Status:    Running
├── Contract:      Operational
├── Funds:         Secure
└── Monitoring:    Active

📝 FOLLOW-UP:
├── [ ] Root cause fix deployed
├── [ ] Post-mortem completed
└── [ ] Process improvements implemented

═══════════════════════════════════════════════════════════════
```

---

*"Hazırlık, paniği önler."*
