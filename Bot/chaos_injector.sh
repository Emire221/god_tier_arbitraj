#!/usr/bin/env bash
# ============================================================================
#  CHAOS INJECTOR v1.1 — Uçtan Uca Cehennem Simülasyonu (Cross-Platform)
# ============================================================================

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [ -f "$SCRIPT_DIR/.env" ]; then
    set -a
    source "$SCRIPT_DIR/.env"
    set +a
fi

RPC_URL="${RPC_HTTP_URL:-http://127.0.0.1:8545}"
POOL_A="${POOL_A_ADDRESS:?POOL_A_ADDRESS .env dosyasında tanımlı olmalı}"
POOL_B="${POOL_B_ADDRESS:?POOL_B_ADDRESS .env dosyasında tanımlı olmalı}"
OWNER_PK="${PRIVATE_KEY:?PRIVATE_KEY .env dosyasında tanımlı olmalı}"
OWNER_ADDRESS=$(cast wallet address "$OWNER_PK" 2>/dev/null || echo "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266")
WETH_ADDRESS="${WETH_ADDRESS:-0x4200000000000000000000000000000000000006}"
SLOT0_POSITION="0x0000000000000000000000000000000000000000000000000000000000000000"

MAX_DEVIATION_PCT=5
CYCLE_DELAY_SECS=2
MAX_CYCLES=0
LOG_FILE="$SCRIPT_DIR/chaos_log.txt"
CYCLE_COUNT=0

log_info()  { echo -e "${CYAN}[KAOS]${NC} $(date '+%H:%M:%S') $*"; }
log_ok()    { echo -e "${GREEN}[  OK]${NC} $(date '+%H:%M:%S') $*"; }
log_warn()  { echo -e "${YELLOW}[UYAR]${NC} $(date '+%H:%M:%S') $*"; }
log_fatal() { echo -e "${RED}${BOLD}[ÖLÜM]${NC} $(date '+%H:%M:%S') $*"; }

# ─── PYTHON KOMUTUNU BUL (Windows/Linux Uyumluluğu) ───
if command -v python &> /dev/null; then
    PYTHON_CMD="python"
elif command -v python3 &> /dev/null; then
    PYTHON_CMD="python3"
else
    log_fatal "Python bulunamadı! Lütfen sisteme Python yükleyin."
    exit 1
fi

get_weth_balance() {
    local result
    result=$(cast call "$WETH_ADDRESS" "balanceOf(address)(uint256)" "$1" --rpc-url "$RPC_URL" 2>/dev/null) || echo "0"
    echo "$result" | tr -d '\r\n '
}

get_sqrt_price_x96() {
    local raw
    raw=$(cast storage "$1" "$SLOT0_POSITION" --rpc-url "$RPC_URL" 2>/dev/null) || { echo "0"; return; }
    echo "$raw" | tr -d '\r\n '
}

manipulate_price() {
    local pool="$1"
    local pool_name="$2"
    local current_slot0

    current_slot0=$(get_sqrt_price_x96 "$pool")

    if [ "$current_slot0" = "0" ] || [ -z "$current_slot0" ]; then
        log_warn "$pool_name: slot0 okunamadı, atlanıyor"
        return
    fi

    local direction=$((RANDOM % 2))
    local deviation=$((RANDOM % MAX_DEVIATION_PCT + 1))

    # Python hesaplaması (Heredoc ile syntax hatası riskini sıfırlar)
    local new_slot0
    new_slot0=$($PYTHON_CMD - <<EOF
import sys
slot0_hex = '$current_slot0'.strip()
if slot0_hex.startswith('0x') or slot0_hex.startswith('0X'):
    slot0_hex = slot0_hex[2:]
if not slot0_hex:
    sys.exit(1)
try:
    slot0_int = int(slot0_hex, 16)
except ValueError:
    sys.exit(1)

mask_160 = (1 << 160) - 1
sqrt_price = slot0_int & mask_160
upper_bits = slot0_int & ~mask_160

deviation_factor = $deviation / 200.0
if $direction == 0:
    new_sqrt = int(sqrt_price * (1.0 + deviation_factor))
else:
    new_sqrt = int(sqrt_price * (1.0 - deviation_factor))

new_sqrt = min(new_sqrt, mask_160)
new_sqrt = max(new_sqrt, 1)

new_slot0 = upper_bits | new_sqrt
print(hex(new_slot0))
EOF
) || { log_warn "$pool_name: Python hesaplama hatası"; return; }

    local padded
    padded=$($PYTHON_CMD - <<EOF
val = '$new_slot0'.strip()
if val.startswith('0x'): val = val[2:]
print('0x' + val.zfill(64))
EOF
)

    local dir_str="AŞAĞI ↓ (-%${deviation})"
    if [ "$direction" -eq 0 ]; then dir_str="YUKARI ↑ (+%${deviation})"; fi

    log_info "$pool_name: Fiyat $dir_str manipüle ediliyor..."

    cast rpc anvil_setStorageAt "$pool" "$SLOT0_POSITION" "$padded" --rpc-url "$RPC_URL" > /dev/null 2>&1 || {
        log_warn "$pool_name: Storage yazma başarısız"
        return
    }
    log_ok "$pool_name: $dir_str uygulandı"
}

echo ""
echo -e "${RED}${BOLD}╔══════════════════════════════════════════════════════════════════╗${NC}"
echo -e "${RED}${BOLD}║         🔥  KAOS ENJEKTÖRü v1.1 — CEHENNEM SİMÜLASYONU  🔥     ║${NC}"
echo -e "${RED}${BOLD}╚══════════════════════════════════════════════════════════════════╝${NC}"
echo ""

log_info "Anvil bağlantısı kontrol ediliyor..."
if ! cast block-number --rpc-url "$RPC_URL" > /dev/null 2>&1; then
    log_fatal "Anvil'e bağlanılamıyor!"
    exit 1
fi
log_ok "Anvil bağlantısı başarılı"

INITIAL_BALANCE=$(get_weth_balance "$OWNER_ADDRESS")
log_info "Başlangıç WETH bakiyesi: $INITIAL_BALANCE wei"

log_info "🔥 Kaos Enjeksiyonu başlıyor..."
echo ""

while true; do
    CYCLE_COUNT=$((CYCLE_COUNT + 1))
    if [ "$MAX_CYCLES" -gt 0 ] && [ "$CYCLE_COUNT" -gt "$MAX_CYCLES" ]; then break; fi

    echo -e "${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    log_info "📍 Döngü #$CYCLE_COUNT"

    BALANCE_BEFORE=$(get_weth_balance "$OWNER_ADDRESS")
    log_info "💰 Bakiye (ÖNCE): $BALANCE_BEFORE wei"

    manipulate_price "$POOL_A" "Havuz-A (Uniswap V3)"
    manipulate_price "$POOL_B" "Havuz-B (Aerodrome)"

    log_info "⏳ Bot tepkisi bekleniyor (${CYCLE_DELAY_SECS}s)..."
    sleep "$CYCLE_DELAY_SECS"

    cast rpc evm_mine --rpc-url "$RPC_URL" > /dev/null 2>&1 || true

    BALANCE_AFTER=$(get_weth_balance "$OWNER_ADDRESS")
    log_info "💰 Bakiye (SONRA): $BALANCE_AFTER wei"

    BALANCE_CHECK=$($PYTHON_CMD - <<EOF
before = int('$BALANCE_BEFORE'.strip() or '0')
after  = int('$BALANCE_AFTER'.strip() or '0')
diff   = after - before
if diff > 0: print(f'KAR|+{diff} wei')
elif diff == 0: print('ESIT|0 wei')
else: print(f'ZARAR|{diff} wei')
EOF
)

    STATUS=$(echo "$BALANCE_CHECK" | cut -d'|' -f1)
    DIFF=$(echo "$BALANCE_CHECK" | cut -d'|' -f2)

    if [ "$STATUS" = "KAR" ]; then
        log_ok "✅ Bot KÂR etti! ($DIFF)"
    elif [ "$STATUS" = "ESIT" ]; then
        log_info "➖ Bakiye değişmedi (fırsat yetersizdi)"
    else
        echo -e "${RED}${BOLD}💀 KRİTİK HATA: BOT ZARAR ETTİ! ($DIFF)${NC}"
        exit 1
    fi
done