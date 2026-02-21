// SPDX-License-Identifier: MIT
pragma solidity ^0.8.27;

// ══════════════════════════════════════════════════════════════════════════════
//
//   ARBITRAJ BOTU v8.0 — "Kuantum Kalkan" Kontratı
//   Base Network — Gas-Minimized Flash Swap Arbitrage Engine
//
//   v7 → v8 Kritik Yükseltmeler:
//
//   1. SANDVİÇ SALDIRISI KORUMASI (minProfit)
//      • Bot off-chain hesapladığı kârın toleranslı halini (uint128 minProfit)
//        calldata içinde gönderir.
//      • Kontrat, işlem sonunda kârın minProfit'e ulaşıp ulaşmadığını
//        KESİNLİKLE kontrol eder. Ulaşılmazsa tüm TX revert edilir.
//      • 1 wei kâr ile işlem kapanması artık İMKANSIZ.
//
//   2. AERODROME SLİPSTREAM (Concentrated Liquidity) UYUMU
//      • Eski V2 stil swap (getAmountOut + statik swap) tamamen SİLİNDİ.
//      • Slipstream = Uniswap V3 fork → V3 callback mekanizması implemente.
//      • Hem UniV3 hem Slipstream aynı uniswapV3SwapCallback'i kullanır.
//      • İki farklı callback kaynağı transient storage ile ayırt edilir.
//
//   3. ON-CHAIN I/O ve GAS İSRAFININ ÖNLENMESİ
//      • token0() ve token1() on-chain okumaları tamamen SİLİNDİ.
//      • owedToken ve receivedToken adresleri bot tarafından off-chain
//        belirlenir ve calldata içinde gönderilir.
//      • İşlem başına ~5000+ gas tasarrufu.
//
//   4. GENİŞLETİLMİŞ CALLDATA MİMARİSİ (130 byte)
//      • Eski 73 byte → yeni 130 byte (hâlâ ABI'den kompakt)
//      • Eklenen alanlar: owedToken, receivedToken, aeroDirection, minProfit
//      • Kaldırılan maliyetler: token0(), token1(), getAmountOut() çağrıları
//
// ══════════════════════════════════════════════════════════════════════════════
//
//   KOMPAKT CALLDATA FORMATI (130 byte — ABI kodlama YOK)
//
//   Offset   Boyut   Alan
//   ─────────────────────────────────────────────────────
//   0x00     20 B    Pool A adresi (Uniswap V3 — flash swap kaynağı)
//   0x14     20 B    Pool B adresi (Aerodrome Slipstream — satış hedefi)
//   0x28     20 B    owedToken (borçlu/kâr token adresi)
//   0x3C     20 B    receivedToken (alınan/input token adresi)
//   0x50     32 B    Miktar (uint256, big-endian)
//   0x70      1 B    UniV3 Yön (0x00 = zeroForOne=true, 0x01 = false)
//   0x71      1 B    Slipstream Yön (0x00 = zeroForOne=true, 0x01 = false)
//   0x72     16 B    minProfit (uint128, big-endian)
//   ─────────────────────────────────────────────────────
//   TOPLAM  130 B    (Eski: 73 B + on-chain çağrılar → şimdi saf calldata)
//
// ══════════════════════════════════════════════════════════════════════════════
//
//   EIP-1153 TRANSIENT STORAGE SLOT HARİTASI
//
//   Slot     İçerik
//   ─────────────────────────────────
//   0x00     expectedPool   — UniV3 callback çağrıcı doğrulaması
//   0x01     aeroPool       — Slipstream callback çağrıcı doğrulaması
//   0x02     aeroDirection  — Slipstream swap yönü (zeroForOne)
//   0x03     owedToken      — Borçlu/kâr token adresi
//   0x04     receivedToken  — Alınan/satılan token adresi
//   0xFF     reentrancy     — kilit (1 = kilitli, 0 = açık)
//
// ══════════════════════════════════════════════════════════════════════════════

// ── CUSTOM ERRORS ────────────────────────────────────────────────────────────

/// @dev Çağrıcı yetkili değil (owner değil)
error Unauthorized();

/// @dev Callback çağrıcısı beklenen havuz değil (ne UniV3 ne Slipstream)
error InvalidCaller();

/// @dev Arbitraj sonrası kâr elde edilemedi (bakiye artmadı)
error NoProfitRealized();

/// @dev Kâr, belirlenen minimum kâr eşiğinin (minProfit) altında kaldı
/// @dev Sandviç saldırısı veya likidite kayması tespit edildiğinde tetiklenir
error InsufficientProfit();

/// @dev Reentrancy tespit edildi (transient storage kilidi)
error Locked();

/// @dev İşlem miktarı sıfır
error ZeroAmount();

/// @dev ERC20 transfer başarısız
error TransferFailed();

// ── MINIMAL INTERFACES ───────────────────────────────────────────────────────

interface IERC20 {
    function balanceOf(address account) external view returns (uint256);
    function transfer(address to, uint256 amount) external returns (bool);
}

/// @dev Uniswap V3 Pool arayüzü — flash swap kaynağı
interface IUniswapV3Pool {
    function swap(
        address recipient,
        bool zeroForOne,
        int256 amountSpecified,
        uint160 sqrtPriceLimitX96,
        bytes calldata data
    ) external returns (int256 amount0, int256 amount1);
}

/// @dev Aerodrome Slipstream (Concentrated Liquidity) Pool arayüzü
///      UniV3 fork — aynı swap imzası, aynı callback mekanizması
interface ICLPool {
    function swap(
        address recipient,
        bool zeroForOne,
        int256 amountSpecified,
        uint160 sqrtPriceLimitX96,
        bytes calldata data
    ) external returns (int256 amount0, int256 amount1);
}

// ══════════════════════════════════════════════════════════════════════════════
//                            ANA KONTRAT
// ══════════════════════════════════════════════════════════════════════════════

contract ArbitrajBotu {

    // ─────────────────────────────────────────────────────────────────────────
    //  IMMUTABLE — bytecode'a gömülü, SLOAD = 0 gas
    // ─────────────────────────────────────────────────────────────────────────

    /// @notice Kontrat sahibi. Constructor'da atanır, değiştirilemez.
    ///         Bytecode'da saklanır → okuma maliyeti ~3 gas (SLOAD: 2100 gas).
    address public immutable owner;

    // ─────────────────────────────────────────────────────────────────────────
    //  CONSTANTS — Uniswap V3 / Slipstream sqrt price limits
    // ─────────────────────────────────────────────────────────────────────────

    /// @dev zeroForOne=true → minimum fiyat sınırı (TickMath.MIN_SQRT_RATIO + 1)
    uint160 private constant MIN_SQRT_RATIO_PLUS_1 = 4295128740;

    /// @dev zeroForOne=false → maksimum fiyat sınırı (TickMath.MAX_SQRT_RATIO - 1)
    uint160 private constant MAX_SQRT_RATIO_MINUS_1 =
        1461446703485210103287273052203988822378723970341;

    // ─────────────────────────────────────────────────────────────────────────
    //  EVENTS
    // ─────────────────────────────────────────────────────────────────────────

    event ArbitrageExecuted(
        address indexed poolA,
        address indexed poolB,
        uint256 amountIn,
        uint256 profit
    );
    event EmergencyTokenWithdraw(
        address indexed token, uint256 amount, address indexed to
    );
    event EmergencyETHWithdraw(uint256 amount, address indexed to);

    // ─────────────────────────────────────────────────────────────────────────
    //  CONSTRUCTOR — owner immutable olarak bytecode'a yazılır
    // ─────────────────────────────────────────────────────────────────────────

    constructor() {
        owner = msg.sender;
    }

    // ═════════════════════════════════════════════════════════════════════════
    //  CORE GİRİŞ NOKTASI — 130-BYTE KOMPAKT CALLDATA (fallback)
    // ═════════════════════════════════════════════════════════════════════════
    //
    //  Rust botu 130 byte sıkıştırılmış veriyi gönderir:
    //    [PoolA:20] + [PoolB:20] + [owedToken:20] + [receivedToken:20]
    //    + [Miktar:32] + [UniYön:1] + [AeroYön:1] + [minProfit:16]
    //
    //  • Fonksiyon seçici YOK — fallback() devralır
    //  • ABI kodlama YOK — calldataload ile ham byte okuması
    //  • token0()/token1() çağrısı YOK — adresler calldata'dan gelir
    //
    //  Akış:
    //    1. Owner kontrolü (immutable — bytecode, ~3 gas)
    //    2. Reentrancy kilidi (TSTORE — geçici hafıza)
    //    3. Calldata çözümleme (assembly — 130 byte)
    //    4. TSTORE callback bağlamı (EIP-1153)
    //    5. Bakiye oku (ÖNCE)
    //    6. Flash swap tetikle → UniV3 callback → Slipstream swap → geri öde
    //    7. Bakiye oku (SONRA)
    //    8. Sandviç koruması: kâr >= minProfit kontrolü
    //    9. Kâr varsa ve yeterliyse sahibine gönder, yoksa revert
    //
    // ═════════════════════════════════════════════════════════════════════════

    fallback() external {
        // ── 1. SAHİPLİK KONTROLÜ ─────────────────────────────────────────
        if (msg.sender != owner) revert Unauthorized();

        // ── 2. REENTRANCY KİLİDİ (EIP-1153 Transient Storage) ────────────
        uint256 locked;
        assembly { locked := tload(0xFF) }
        if (locked != 0) revert Locked();
        assembly { tstore(0xFF, 1) }

        // ── 3. CALLDATA ÇÖZÜMLEME (Assembly — 130 byte saf okuma) ────────
        address poolA;
        address poolB;
        address owedToken;
        address receivedToken;
        uint256 amount;
        uint256 uniDirection;
        uint256 aeroDirection;
        uint256 minProfit;

        assembly {
            poolA         := shr(96,  calldataload(0x00))  // [0..20]    Pool A (UniV3)
            poolB         := shr(96,  calldataload(0x14))  // [20..40]   Pool B (Slipstream)
            owedToken     := shr(96,  calldataload(0x28))  // [40..60]   Borçlu/kâr token
            receivedToken := shr(96,  calldataload(0x3C))  // [60..80]   Alınan/input token
            amount        := calldataload(0x50)             // [80..112]  Miktar (uint256)
            uniDirection  := shr(248, calldataload(0x70))  // [112]      UniV3 yön (1 byte)
            aeroDirection := shr(248, calldataload(0x71))  // [113]      Slipstream yön (1 byte)
            minProfit     := shr(128, calldataload(0x72))  // [114..130] minProfit (uint128)
        }

        if (amount == 0) revert ZeroAmount();

        bool zeroForOne = (uniDirection == 0);

        // ── 4. TSTORE — Callback bağlamını geçici hafızaya yaz ───────────
        //    UniV3 ve Slipstream callback'leri bu slot'lardan okuyacak.
        //    TX bittiğinde otomatik silinir → gas iadesi.
        assembly {
            tstore(0x00, poolA)          // Slot 0: expectedPool (UniV3 callback güvenliği)
            tstore(0x01, poolB)          // Slot 1: aeroPool (Slipstream callback güvenliği)
            tstore(0x02, aeroDirection)  // Slot 2: Slipstream swap yönü
            tstore(0x03, owedToken)      // Slot 3: Borçlu/kâr token adresi
            tstore(0x04, receivedToken)  // Slot 4: Alınan/input token adresi
        }

        // ── 5. BAKİYE KONTROLÜ — ÖNCE ───────────────────────────────────
        uint256 balBefore = IERC20(owedToken).balanceOf(address(this));

        // ── 6. FLASH SWAP TETİKLE ────────────────────────────────────────
        //    UniV3 flash swap: token'lar ÖNCE gönderilir,
        //    sonra uniswapV3SwapCallback tetiklenir.
        //    Callback içinde Slipstream satışı + UniV3 borç ödeme yapılır.
        uint160 priceLimit = zeroForOne
            ? MIN_SQRT_RATIO_PLUS_1
            : MAX_SQRT_RATIO_MINUS_1;

        IUniswapV3Pool(poolA).swap(
            address(this),       // recipient: biz
            zeroForOne,          // swap yönü
            int256(amount),      // exact input
            priceLimit,          // fiyat sınırı
            hex"01"              // data: ≥1 byte → callback tetiklenir (TLOAD kullanılır)
        );

        // ── 7. BAKİYE KONTROLÜ — SONRA ──────────────────────────────────
        uint256 balAfter = IERC20(owedToken).balanceOf(address(this));

        // ── 8. SANDVİÇ KORUMASI — İki Aşamalı Doğrulama ────────────────
        //    Aşama 1: Kâr var mı? (bakiye artmış olmalı)
        if (balAfter <= balBefore) revert NoProfitRealized();

        uint256 profit = balAfter - balBefore;

        //    Aşama 2: Kâr yeterli mi? (minProfit eşiği)
        //    Bot'un off-chain hesapladığı kârın toleranslı hali.
        //    Sandviç saldırısında kâr düştüğünde burası revert atar.
        if (profit < minProfit) revert InsufficientProfit();

        // ── 9. KÂRI SAHİBE GÖNDER ───────────────────────────────────────
        _safeTransfer(owedToken, owner, profit);

        // ── 10. TRANSIENT STORAGE TEMİZLİĞİ + EVENT ─────────────────────
        assembly {
            tstore(0x00, 0)  // expectedPool
            tstore(0x01, 0)  // aeroPool
            tstore(0x02, 0)  // aeroDirection
            tstore(0x03, 0)  // owedToken
            tstore(0x04, 0)  // receivedToken
            tstore(0xFF, 0)  // reentrancy kilidi
        }

        emit ArbitrageExecuted(poolA, poolB, amount, profit);
    }

    // ═════════════════════════════════════════════════════════════════════════
    //  CALLBACK — Uniswap V3 / Aerodrome Slipstream Ortak Geri Çağrısı
    // ═════════════════════════════════════════════════════════════════════════
    //
    //  Bu tek fonksiyon İKİ farklı havuzdan gelen callback'leri yönetir:
    //
    //  A) UniV3 Callback (msg.sender == expectedPool):
    //     1. Borçlu ve alınan miktarları belirle
    //     2. Alınan token'larla Slipstream'de swap yap
    //     3. Slipstream callback'i tetiklenir (B)
    //     4. UniV3 borcunu geri öde
    //
    //  B) Slipstream Callback (msg.sender == aeroPool):
    //     1. Borçlu miktarı belirle (pozitif delta)
    //     2. receivedToken'ı Slipstream'e transfer et (borç öde)
    //
    //  Güvenlik: Her iki yol da transient storage ile doğrulanır.
    //            Ne UniV3 ne Slipstream olmayan çağrıcılar reddedilir.
    //
    // ═════════════════════════════════════════════════════════════════════════

    function uniswapV3SwapCallback(
        int256 amount0Delta,
        int256 amount1Delta,
        bytes calldata /* data — kullanılmıyor, TLOAD kullanılıyor */
    ) external {
        // ── TRANSIENT STORAGE'DAN BAĞLAM OKU ─────────────────────────────
        address expectedPool;
        address aeroPool;
        uint256 aeroDir;
        address owedToken;
        address receivedToken;

        assembly {
            expectedPool  := tload(0x00)
            aeroPool      := tload(0x01)
            aeroDir       := tload(0x02)
            owedToken     := tload(0x03)
            receivedToken := tload(0x04)
        }

        // ═════════════════════════════════════════════════════════════════
        //  YOL A: UniV3 Flash Swap Callback
        // ═════════════════════════════════════════════════════════════════
        if (msg.sender == expectedPool) {
            // ── Borçlu ve alınan miktarları belirle ──────────────────────
            uint256 amountOwed;
            uint256 amountReceived;

            if (amount0Delta > 0) {
                // token0 borçlu, token1 alındı
                amountOwed     = uint256(amount0Delta);
                amountReceived = uint256(-amount1Delta);
            } else {
                // token1 borçlu, token0 alındı
                amountOwed     = uint256(amount1Delta);
                amountReceived = uint256(-amount0Delta);
            }

            // ── Slipstream'de Sat (V3 Concentrated Liquidity Swap) ───────
            //    Alınan token'ları (receivedToken) Slipstream'de owedToken'a çevir.
            //    Slipstream, UniV3 fork — aynı swap mekanizması.
            //    Slipstream callback'i (Yol B) tetiklenecek ve borç ödenecek.
            bool aeroZeroForOne = (aeroDir == 0);
            uint160 aeroLimit = aeroZeroForOne
                ? MIN_SQRT_RATIO_PLUS_1
                : MAX_SQRT_RATIO_MINUS_1;

            ICLPool(aeroPool).swap(
                address(this),           // recipient: biz
                aeroZeroForOne,          // Slipstream swap yönü
                int256(amountReceived),  // exact input (alınan miktar)
                aeroLimit,               // fiyat sınırı
                hex"01"                  // data: ≥1 byte → callback tetiklenir (TLOAD kullanılır)
            );

            // ── UniV3 Borcunu Öde ────────────────────────────────────────
            //    owedToken → UniV3 havuzuna borçlu miktarı transfer et.
            //    Slipstream swap'tan elde edilen owedToken bu ödemeye yeter.
            //    Kalan fazlalık = kâr (fallback() bakiye kontrolü ile doğrular).
            _safeTransfer(owedToken, msg.sender, amountOwed);

        // ═════════════════════════════════════════════════════════════════
        //  YOL B: Slipstream (Aerodrome CL) Callback
        // ═════════════════════════════════════════════════════════════════
        } else if (msg.sender == aeroPool) {
            // ── Slipstream'e Borç Öde ────────────────────────────────────
            //    Pozitif delta = borçlu miktar. receivedToken'ı transfer et.
            //    receivedToken = UniV3'ten aldığımız token = Slipstream'in input'u
            uint256 amountOwedToSlipstream;
            if (amount0Delta > 0) {
                amountOwedToSlipstream = uint256(amount0Delta);
            } else {
                amountOwedToSlipstream = uint256(amount1Delta);
            }
            _safeTransfer(receivedToken, msg.sender, amountOwedToSlipstream);

        // ═════════════════════════════════════════════════════════════════
        //  REDDET: Bilinmeyen Çağrıcı
        // ═════════════════════════════════════════════════════════════════
        } else {
            revert InvalidCaller();
        }
    }

    // ═════════════════════════════════════════════════════════════════════════
    //  ACİL DURUM — Token ve ETH Kurtarma
    // ═════════════════════════════════════════════════════════════════════════

    /// @notice Kontrattaki tüm token bakiyesini sahibine çek
    function withdrawToken(address token) external {
        if (msg.sender != owner) revert Unauthorized();
        uint256 bal = IERC20(token).balanceOf(address(this));
        if (bal == 0) revert ZeroAmount();
        _safeTransfer(token, owner, bal);
        emit EmergencyTokenWithdraw(token, bal, owner);
    }

    /// @notice Kontrattaki tüm ETH bakiyesini sahibine çek
    function withdrawETH() external {
        if (msg.sender != owner) revert Unauthorized();
        uint256 bal = address(this).balance;
        if (bal == 0) revert ZeroAmount();
        (bool ok, ) = owner.call{value: bal}("");
        if (!ok) revert TransferFailed();
        emit EmergencyETHWithdraw(bal, owner);
    }

    // ═════════════════════════════════════════════════════════════════════════
    //  VIEW — Bakiye Sorgulama
    // ═════════════════════════════════════════════════════════════════════════

    /// @notice Kontrat'ın belirli bir token bakiyesini döndür
    function getBalance(address token) external view returns (uint256) {
        return IERC20(token).balanceOf(address(this));
    }

    // ═════════════════════════════════════════════════════════════════════════
    //  INTERNAL — Güvenli ERC20 Transfer (Non-Standard Token Desteği)
    // ═════════════════════════════════════════════════════════════════════════

    /// @dev Low-level call ile ERC20 transfer. USDT gibi bool dönmeyen
    ///      token'ları da destekler. Başarısız olursa revert.
    function _safeTransfer(address token, address to, uint256 amt) internal {
        (bool ok, bytes memory ret) = token.call(
            abi.encodeWithSelector(IERC20.transfer.selector, to, amt)
        );
        if (!ok || (ret.length > 0 && !abi.decode(ret, (bool)))) {
            revert TransferFailed();
        }
    }

    // ═════════════════════════════════════════════════════════════════════════
    //  RECEIVE — ETH kabul (WETH unwrap iadesi vb.)
    // ═════════════════════════════════════════════════════════════════════════

    receive() external payable {}
}
