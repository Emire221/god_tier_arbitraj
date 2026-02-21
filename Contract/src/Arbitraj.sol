// SPDX-License-Identifier: MIT
pragma solidity ^0.8.27;

// ══════════════════════════════════════════════════════════════════════════════
//
//   ARBITRAJ BOTU v9.0 — "Kuantum Kalkan II" Kontratı
//   Base Network — Gas-Minimized Flash Swap Arbitrage Engine
//
//   v8 → v9 Kritik Güvenlik Yükseltmeleri:
//
//   1. EXECUTOR / ADMIN ROL AYRIMI (Tek Nokta Hatası Giderimi)
//      • Eski: Tek owner hem arbitraj yürütme hem fon çekme yetkisine sahipti.
//      • Yeni: İki ayrı immutable rol:
//        - executor: Sadece fallback() çağırabilir (sıcak cüzdan, düşük bakiye)
//        - admin: Sadece withdrawToken/withdrawETH çağırabilir (soğuk cüzdan/multisig)
//      • Kâr kontrat içinde birikir, admin periyodik olarak çeker.
//      • Sunucu hacklenip executor key çalınsa bile sermaye güvende kalır.
//
//   2. İŞLEM BAYATLAMA KORUMASI (Deadline Block)
//      • Calldata'ya 4-byte deadlineBlock (uint32) eklendi → 134 byte.
//      • block.number > deadlineBlock ise TX otomatik revert olur.
//      • Mempool'da takılan stale TX'lerin kötü koşullarda çalışması engellenir.
//
//   v7 → v8 (korunuyor):
//   ✓ Sandviç saldırısı koruması (minProfit)
//   ✓ Aerodrome Slipstream (CL) uyumu
//   ✓ On-chain I/O eliminasyonu (token0/token1 çağrısı YOK)
//   ✓ 130-byte kompakt calldata mimarisi (şimdi 134-byte)
//
// ══════════════════════════════════════════════════════════════════════════════
//
//   KOMPAKT CALLDATA FORMATI (134 byte — ABI kodlama YOK)
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
//   0x82      4 B    deadlineBlock (uint32, big-endian) ← v9.0 YENİ
//   ─────────────────────────────────────────────────────
//   TOPLAM  134 B    (v8: 130 B → v9: +4 B deadline koruması)
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

/// @dev İşlem deadline blok numarasını aştı (stale TX koruması)
error DeadlineExpired();

/// @dev Sıfır adres (address(0)) kullanılamaz
error ZeroAddress();

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

    /// @notice Arbitraj yürütme yetkisi. Sıcak cüzdan — düşük bakiyeli.
    ///         Sadece fallback() çağırabilir. Fon çekme yetkisi YOKTUR.
    ///         Bytecode'da saklanır → okuma maliyeti ~3 gas.
    address public immutable executor;

    /// @notice Fon yönetimi yetkisi. Soğuk cüzdan veya multisig.
    ///         Sadece withdrawToken/withdrawETH çağırabilir.
    ///         Arbitraj yürütme yetkisi YOKTUR.
    ///         Bytecode'da saklanır → okuma maliyeti ~3 gas.
    address public immutable admin;

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
    //  CONSTRUCTOR — executor ve admin immutable olarak bytecode'a yazılır
    // ─────────────────────────────────────────────────────────────────────

    /// @param _executor Arbitraj yürütme adresi (sıcak cüzdan)
    /// @param _admin    Fon yönetimi adresi (soğuk cüzdan/multisig)
    constructor(address _executor, address _admin) {
        if (_executor == address(0) || _admin == address(0)) revert ZeroAddress();
        executor = _executor;
        admin = _admin;
    }

    // ═════════════════════════════════════════════════════════════════════════
    //  CORE GİRİŞ NOKTASI — 134-BYTE KOMPAKT CALLDATA (fallback)
    // ═════════════════════════════════════════════════════════════════════════
    //
    //  Rust botu 134 byte sıkıştırılmış veriyi gönderir:
    //    [PoolA:20] + [PoolB:20] + [owedToken:20] + [receivedToken:20]
    //    + [Miktar:32] + [UniYön:1] + [AeroYön:1] + [minProfit:16] + [deadlineBlock:4]
    //
    //  • Fonksiyon seçici YOK — fallback() devralır
    //  • ABI kodlama YOK — calldataload ile ham byte okuması
    //  • token0()/token1() çağrısı YOK — adresler calldata'dan gelir
    //
    //  Akış:
    //    1. Executor kontrolü (immutable — bytecode, ~3 gas)
    //    2. Reentrancy kilidi (TSTORE — geçici hafıza)
    //    3. Calldata çözümleme (assembly — 134 byte)
    //    4. Deadline kontrolü (block.number <= deadlineBlock)
    //    5. TSTORE callback bağlamı (EIP-1153)
    //    6. Bakiye oku (ÖNCE)
    //    7. Flash swap tetikle → UniV3 callback → Slipstream swap → geri öde
    //    8. Bakiye oku (SONRA)
    //    9. Sandviç koruması: kâr >= minProfit kontrolü
    //   10. Kâr kontrat içinde kalır (admin periyodik çeker)
    //
    // ═════════════════════════════════════════════════════════════════════════

    fallback() external {
        // ── 1. EXECUTOR KONTROLÜ ─────────────────────────────────────────
        if (msg.sender != executor) revert Unauthorized();

        // ── 2. REENTRANCY KİLİDİ (EIP-1153 Transient Storage) ────────────
        uint256 locked;
        assembly { locked := tload(0xFF) }
        if (locked != 0) revert Locked();
        assembly { tstore(0xFF, 1) }

        // ── 3. CALLDATA ÇÖZÜMLEME (Assembly — 134 byte saf okuma) ────────
        address poolA;
        address poolB;
        address owedToken;
        address receivedToken;
        uint256 amount;
        uint256 uniDirection;
        uint256 aeroDirection;
        uint256 minProfit;
        uint256 deadlineBlock;

        assembly {
            poolA         := shr(96,  calldataload(0x00))  // [0..20]    Pool A (UniV3)
            poolB         := shr(96,  calldataload(0x14))  // [20..40]   Pool B (Slipstream)
            owedToken     := shr(96,  calldataload(0x28))  // [40..60]   Borçlu/kâr token
            receivedToken := shr(96,  calldataload(0x3C))  // [60..80]   Alınan/input token
            amount        := calldataload(0x50)             // [80..112]  Miktar (uint256)
            uniDirection  := shr(248, calldataload(0x70))  // [112]      UniV3 yön (1 byte)
            aeroDirection := shr(248, calldataload(0x71))  // [113]      Slipstream yön (1 byte)
            minProfit     := shr(128, calldataload(0x72))  // [114..130] minProfit (uint128)
            deadlineBlock := shr(224, calldataload(0x82))  // [130..134] deadlineBlock (uint32)
        }

        if (amount == 0) revert ZeroAmount();

        // ── 3.5. DEADLINE KONTROLÜ (Stale TX koruması) ───────────────────
        //    Hedeflenen blokta çalışmayan işlem otomatik revert olur.
        //    Mempool'da takılan TX'lerin kötü koşullarda yürütme riski engellenir.
        if (block.number > deadlineBlock) revert DeadlineExpired();

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

        // ── 9. KÂR KONTRAT İÇİNDE KALIR ───────────────────────────────
        //    v9.0: Kâr otomatik olarak dışarı gönderilmez.
        //    Admin (soğuk cüzdan/multisig) withdrawToken ile periyodik çeker.
        //    Böylece executor key çalınsa bile sermaye güvende kalır.

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

    /// @notice Kontrattaki tüm token bakiyesini admin'e çek
    /// @dev Sadece admin (soğuk cüzdan/multisig) çağırabilir
    function withdrawToken(address token) external {
        if (msg.sender != admin) revert Unauthorized();
        uint256 bal = IERC20(token).balanceOf(address(this));
        if (bal == 0) revert ZeroAmount();
        _safeTransfer(token, admin, bal);
        emit EmergencyTokenWithdraw(token, bal, admin);
    }

    /// @notice Kontrattaki tüm ETH bakiyesini admin'e çek
    /// @dev Sadece admin (soğuk cüzdan/multisig) çağırabilir
    function withdrawETH() external {
        if (msg.sender != admin) revert Unauthorized();
        uint256 bal = address(this).balance;
        if (bal == 0) revert ZeroAmount();
        (bool ok, ) = admin.call{value: bal}("");
        if (!ok) revert TransferFailed();
        emit EmergencyETHWithdraw(bal, admin);
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
