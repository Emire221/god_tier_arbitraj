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

/// @dev Calldata uzunluğu geçersiz (tam 134 byte olmalı)
/// @dev v13.0: ZeroAmount yerine semantik olarak doğru hata
error InvalidCalldataLength();

/// @dev executor ve admin aynı adres olamaz (rol ayrımı ihlali)
/// @dev v24.0: ZeroAddress yerine semantik olarak doğru hata
error InvalidRoleAssignment();

// (v12.0: PoolNotWhitelisted kaldırıldı — off-chain doğrulama)
// (v22.0: PoolNotWhitelisted geri eklendi — on-chain doğrulama ile güvenlik artırıldı)
error PoolNotWhitelisted();
// (v21.0: BribeFailed kaldırıldı — coinbase bribe kaldırıldı, bribe yalnızca priority fee)

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

    // -----------------------------------------------------------------------
    //  STORAGE — v22.0: On-chain pool whitelist geri eklendi
    //  executor key çalınsa bile sadece whitelistteki havuzlara işlem yapılır.
    //  Gas maliyeti: 1x Warm SLOAD (~100 gas) — güvenlik için kabul edilebilir.
    // -----------------------------------------------------------------------
    mapping(address => bool) public poolWhitelist;

    // ───────────────────────────────────────────────────────────────────────    //  CONSTRUCTOR — executor ve admin immutable olarak bytecode'a yazılır
    // ─────────────────────────────────────────────────────────────────────

    /// @param _executor Arbitraj yürütme adresi (sıcak cüzdan)
    /// @param _admin    Fon yönetimi adresi (soğuk cüzdan/multisig)
    constructor(address _executor, address _admin) {
        if (_executor == address(0) || _admin == address(0)) revert ZeroAddress();
        // v24.0: executor ve admin aynı adres olamaz — rol ayrımı ihlali
        if (_executor == _admin) revert InvalidRoleAssignment();
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

        // ── 1.5. CALLDATA UZUNLUK KONTROLÜ ───────────────────────────────
        //    134 byte'dan farklı veri gelirse (eksik/fazla) anında revert.
        //    Eksik veri EVM tarafından 0 ile doldurulur → minProfit=0, deadlineBlock=0
        //    Bu, MEV botlarına sandviç fırsatı verir. Bu satır o riski kapatır.
        if (msg.data.length != 134) revert InvalidCalldataLength();

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

        // ── 3.5. ON-CHAIN POOL WHITELIST KONTROLÜ (v22.0) ────────────────
        //    Executor key çalınsa bile sadece whitelistteki havuzlara işlem
        //    yapılabilir. İlk erişim Cold SLOAD (~2100 gas), aynı TX'te
        //    tekrar Warm SLOAD (~100 gas). Güvenlik için kabul edilebilir.
        if (!poolWhitelist[poolA]) revert PoolNotWhitelisted();
        if (!poolWhitelist[poolB]) revert PoolNotWhitelisted();

        // ── 3.6. DEADLINE KONTROLÜ (Stale TX koruması) ───────────────────
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

        // ── 5. BAKİYE KONTROLÜ — ÖNCE (Assembly — SLOAD eliminasyonu) ────
        uint256 balBefore;
        assembly {
            // balanceOf(address) selector = 0x70a08231
            mstore(0x00, 0x70a0823100000000000000000000000000000000000000000000000000000000)
            mstore(0x04, address())
            let ok := staticcall(gas(), owedToken, 0x00, 0x24, 0x00, 0x20)
            // v22.0: returndatasize kontrolü — eksik/bozuk dönüş verisi koruması
            if or(iszero(ok), lt(returndatasize(), 0x20)) { revert(0, 0) }
            balBefore := mload(0x00)
        }

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

        // ── 7. BAKİYE KONTROLÜ — SONRA (Assembly — SLOAD eliminasyonu) ───
        uint256 balAfter;
        assembly {
            mstore(0x00, 0x70a0823100000000000000000000000000000000000000000000000000000000)
            mstore(0x04, address())
            let ok := staticcall(gas(), owedToken, 0x00, 0x24, 0x00, 0x20)
            // v22.0: returndatasize kontrolü — eksik/bozuk dönüş verisi koruması
            if or(iszero(ok), lt(returndatasize(), 0x20)) { revert(0, 0) }
            balAfter := mload(0x00)
        }

        // ── 8. SANDVİÇ KORUMASI — İki Aşamalı Doğrulama ────────────────
        //    Aşama 1: Kâr var mı? (bakiye artmış olmalı)
        if (balAfter <= balBefore) revert NoProfitRealized();

        uint256 profit = balAfter - balBefore;

        //    Aşama 2: Kâr yeterli mi? (minProfit eşiği)
        //    Bot'un off-chain hesapladığı kârın toleranslı hali.
        //    Sandviç saldırısında kâr düştüğünde burası revert atar.
        if (profit < minProfit) revert InsufficientProfit();

        // ── 8.5. COINBASE BRIBE — KALDIRILDI (v21.0) ──────────────────
        //    Base (OP Stack) L2'de sequencer sıralaması yalnızca
        //    max_priority_fee_per_gas ile belirlenir. Doğrudan coinbase'e
        //    ETH transferi sequencer tarafından rüşvet olarak işlenmez,
        //    yalnızca gas israfı yaratır. Bribe artık Rust tarafında
        //    priority fee olarak TX'e eklenir.

        // ── 9. KÂR KONTRAT İÇİNDE KALIR ───────────────────────────────
        //    v9.0: Kâr otomatik olarak dışarı gönderilmez.
        //    Admin (soğuk cüzdan/multisig) withdrawToken ile periyodik çeker.
        //    Böylece executor key çalınsa bile sermaye güvende kalır.

        // ── 10. EVENT + REENTRANCY KİLİT TEMİZLİĞİ ────────────────────
        //    v12.0: tstore(0xFF, 0) eklendi — reentrancy kilidi açıkça temizlenir.
        //    EIP-1153 TX sonunda otomatik siler ama açık temizlik:
        //    1. Aynı TX içinde çoklu çağrıyı güvenli kılar (batch call uyumu)
        //    2. Composability için best practice (EIP-7609 tavsiyesi)
        //    Maliyet: ~100 gas warm tstore — kabul edilebilir.
        assembly { tstore(0xFF, 0) }

        emit ArbitrageExecuted(poolA, poolB, amount, profit);
    }

    // ═════════════════════════════════════════════════════════════════════════
    //  CALLBACK — Uniswap V3 / Aerodrome Slipstream Ortak Geri Çağrısı
    // ═════════════════════════════════════════════════════════════════════════
    //
    //  Aşağıdaki fonksiyonlar DEX'lerin callback standartlarına göre
    //  adlandırılmıştır. Her biri aynı iç mantığı (_handleCallback) çağırır:
    //
    //  A) Flash Swap Kaynak (msg.sender == expectedPool):
    //     1. Borçlu ve alınan miktarları belirle
    //     2. Alınan token'larla hedef havuzda (Pool B) swap yap
    //     3. Hedef havuz callback'i tetiklenir (B)
    //     4. Kaynak havuza borcunu geri öde
    //
    //  B) Hedef Havuz Callback (msg.sender == aeroPool):
    //     1. Borçlu miktarı belirle (pozitif delta)
    //     2. receivedToken'ı hedef havuza transfer et (borç öde)
    //
    //  Güvenlik: Her iki yol da transient storage ile doğrulanır.
    //            Tanınmayan çağrıcılar reddedilir (InvalidCaller).
    //
    // ═════════════════════════════════════════════════════════════════════════

    // ── Uniswap V3 Callback ──────────────────────────────────────────────
    //    Tetikleyen DEX'ler:
    //      • Uniswap V3     — doğrudan kendi callback'i
    //      • SushiSwap V3   — UniV3 fork'u, aynı callback imzasını kullanır
    //      • Aerodrome Slipstream (CLPool) — UniV3 fork'u, swap sonrası
    //        uniswapV3SwapCallback çağırır (aerodromeSwapCallback DEĞİL)
    //
    //    Dolayısıyla bu tek fonksiyon 3 DEX'in callback'ini karşılar.
    //    Güvenlik: _handleCallback içinde transient storage doğrulaması yapılır.
    function uniswapV3SwapCallback(
        int256 amount0Delta,
        int256 amount1Delta,
        bytes calldata
    ) external {
        _handleCallback(amount0Delta, amount1Delta);
    }

    // ── PancakeSwap V3 Callback ──────────────────────────────────────────
    //    Tetikleyen DEX: Yalnızca PancakeSwap V3
    //    PancakeSwap V3 havuzları swap sonrası bu fonksiyonu çağırır.
    //    İmza farklı (pancakeV3SwapCallback vs uniswapV3SwapCallback),
    //    mantık aynı — transient storage doğrulaması yapılır.
    function pancakeV3SwapCallback(
        int256 amount0Delta,
        int256 amount1Delta,
        bytes calldata
    ) external {
        _handleCallback(amount0Delta, amount1Delta);
    }

    // ── Aerodrome Slipstream (CL) Callback ───────────────────────────────
    //    DURUM: Şu anda Aerodrome CLPool swap sonrası uniswapV3SwapCallback
    //    çağırır (UniV3 fork'u olduğu için). Bu fonksiyon ÇAĞRILMAZ.
    //
    //    Bu yedek callback Aerodrome'un gelecekte kendi callback adını
    //    kullanmaya geçmesi durumunda korunur. Aktif kullanımda DEĞİLDİR.
    //
    //    Callback haritası (v23.0):
    //      ┌──────────────────────┬──────────────────────────────┐
    //      │ DEX                  │ Çağrılan Callback            │
    //      ├──────────────────────┼──────────────────────────────┤
    //      │ Uniswap V3           │ uniswapV3SwapCallback        │
    //      │ SushiSwap V3         │ uniswapV3SwapCallback        │
    //      │ Aerodrome Slipstream │ uniswapV3SwapCallback        │
    //      │ PancakeSwap V3       │ pancakeV3SwapCallback        │
    //      └──────────────────────┴──────────────────────────────┘
    function aerodromeSwapCallback(
        int256 amount0Delta,
        int256 amount1Delta,
        bytes calldata
    ) external {
        _handleCallback(amount0Delta, amount1Delta);
    }

    // ═════════════════════════════════════════════════════════════════════════
    //  INTERNAL — Ortak Callback Mantığı
    // ═════════════════════════════════════════════════════════════════════════

    function _handleCallback(
        int256 amount0Delta,
        int256 amount1Delta
    ) internal {
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
        //  YOL A: Kaynak Havuz (Flash Swap) Callback
        // ═════════════════════════════════════════════════════════════════
        if (msg.sender == expectedPool) {
            // ── Borçlu ve alınan miktarları belirle ──────────────────────
            uint256 amountOwed;
            uint256 amountReceived;

            if (amount0Delta > 0) {
                // token0 borçlu, token1 alındı
                amountOwed     = uint256(amount0Delta);
                // Güvenli negatif dönüşüm: anormal pozitif delta → 0 (underflow panic yerine)
                amountReceived = amount1Delta < 0 ? uint256(-amount1Delta) : 0;
            } else {
                // token1 borçlu, token0 alındı
                amountOwed     = uint256(amount1Delta);
                // Güvenli negatif dönüşüm: anormal pozitif delta → 0 (underflow panic yerine)
                amountReceived = amount0Delta < 0 ? uint256(-amount0Delta) : 0;
            }

            // ── v24.0: Fee-on-Transfer Token Koruması ────────────────────
            //    Transfer sırasında vergi/kesinti uygulayan tokenlar için
            //    delta'dan gelen miktar ile gerçek bakiye arasında fark olabilir.
            //    Gerçek alınan miktarı bakiyeden oku — delta'ya güvenme.
            {
                uint256 actualBalance;
                assembly {
                    mstore(0x00, 0x70a0823100000000000000000000000000000000000000000000000000000000)
                    mstore(0x04, address())
                    let ok := staticcall(gas(), receivedToken, 0x00, 0x24, 0x00, 0x20)
                    if or(iszero(ok), lt(returndatasize(), 0x20)) { revert(0, 0) }
                    actualBalance := mload(0x00)
                }
                // Gerçek bakiye delta'dan düşükse, fee-on-transfer token.
                // Gerçek bakiyeyi kullan, delta'yı değil.
                if (actualBalance < amountReceived) {
                    amountReceived = actualBalance;
                }
            }

            // ── Hedef Havuzda Sat ────────────────────────────────────────
            //    Alınan token'ları (receivedToken) hedef havuzda owedToken'a çevir.
            //    Hedef havuz callback'i (Yol B) tetiklenecek ve borç ödenecek.
            bool aeroZeroForOne = (aeroDir == 0);
            uint160 aeroLimit = aeroZeroForOne
                ? MIN_SQRT_RATIO_PLUS_1
                : MAX_SQRT_RATIO_MINUS_1;

            ICLPool(aeroPool).swap(
                address(this),           // recipient: biz
                aeroZeroForOne,          // swap yönü
                int256(amountReceived),  // exact input (alınan miktar)
                aeroLimit,               // fiyat sınırı
                hex"01"                  // data: ≥1 byte → callback tetiklenir (TLOAD kullanılır)
            );

            // ── Kaynak Havuz Borcunu Öde ─────────────────────────────────
            _safeTransfer(owedToken, msg.sender, amountOwed);

        // ═════════════════════════════════════════════════════════════════
        //  YOL B: Hedef Havuz Callback
        // ═════════════════════════════════════════════════════════════════
        } else if (msg.sender == aeroPool) {
            // ── Hedef Havuza Borç Öde ────────────────────────────────────
            // Güvenli delta seçimi: pozitif taraf = borçlu miktar
            uint256 amountOwedToTarget;
            if (amount0Delta > 0) {
                amountOwedToTarget = uint256(amount0Delta);
            } else if (amount1Delta > 0) {
                amountOwedToTarget = uint256(amount1Delta);
            } else {
                amountOwedToTarget = 0;
            }
            _safeTransfer(receivedToken, msg.sender, amountOwedToTarget);

        // ═════════════════════════════════════════════════════════════════
        //  REDDET: Bilinmeyen Çağrıcı
        // ═════════════════════════════════════════════════════════════════
        } else {
            revert InvalidCaller();
        }
    }

    // ═════════════════════════════════════════════════════════════════════════════
    //  POOL WHITELIST YÖNETİMİ (v22.0: Geri eklendi — güvenlik öncelikli)
    //  Sadece admin (soğuk cüzdan/multisig) tarafından yönetilir.
    // ═════════════════════════════════════════════════════════════════════════════

    /// @notice Tek bir havuzu whiteliste ekle/çıkar
    /// @param pool Havuz adresi
    /// @param status true = ekle, false = çıkar
    function setPoolWhitelist(address pool, bool status) external {
        if (msg.sender != admin) revert Unauthorized();
        if (pool == address(0)) revert ZeroAddress();
        poolWhitelist[pool] = status;
    }

    /// @notice Birden fazla havuzu whiteliste toplu ekle/çıkar
    /// @param pools Havuz adresleri dizisi
    /// @param status true = ekle, false = çıkar
    function batchSetPoolWhitelist(address[] calldata pools, bool status) external {
        if (msg.sender != admin) revert Unauthorized();
        for (uint256 i; i < pools.length; ++i) {
            if (pools[i] == address(0)) revert ZeroAddress();
            poolWhitelist[pools[i]] = status;
        }
    }

    /// @notice v25.0: Executor'ın keşfettiği havuzları whiteliste EKLEMESİNE izin ver
    /// @dev Sadece EKLEME yapabilir — çıkarma yetkisi YOKTUR (admin ayrıcalığı).
    ///      Otonom keşif motoru yeni havuz bulduğunda bot bu fonksiyonu çağırarak
    ///      admin müdahalesi olmadan havuzu aktif edebilir.
    ///      Güvenlik: Executor zaten sadece whitelistteki havuzlara TX atabilir.
    ///      Executor key çalınsa bile en kötü ihtimal yeni havuz eklenmesidir —
    ///      kontrat fonlarına erişim hâlâ imkansızdır.
    /// @param pool Whiteliste eklenecek havuz adresi
    function executorAddPool(address pool) external {
        if (msg.sender != executor) revert Unauthorized();
        if (pool == address(0)) revert ZeroAddress();
        poolWhitelist[pool] = true;
    }

    /// @notice v25.0: Executor toplu havuz ekleme (otonom keşif batch modu)
    /// @param pools Whiteliste eklenecek havuz adresleri dizisi
    function executorBatchAddPools(address[] calldata pools) external {
        if (msg.sender != executor) revert Unauthorized();
        for (uint256 i; i < pools.length; ++i) {
            if (pools[i] == address(0)) revert ZeroAddress();
            poolWhitelist[pools[i]] = true;
        }
    }

    // ═════════════════════════════════════════════════════════════════════════════
    //  ACİL DURUM — Token ve ETH Kurtarma
    // ═════════════════════════════════════════════════════════════════════════

    /// @notice Kontrattaki tüm token bakiyesini admin'e çek
    /// @dev Sadece admin (soğuk cüzdan/multisig) çağırabilir
    function withdrawToken(address token) external {
        if (msg.sender != admin) revert Unauthorized();
        uint256 bal;
        assembly {
            mstore(0x00, 0x70a0823100000000000000000000000000000000000000000000000000000000)
            mstore(0x04, address())
            let ok := staticcall(gas(), token, 0x00, 0x24, 0x00, 0x20)
            // v22.0: returndatasize kontrolü
            if or(iszero(ok), lt(returndatasize(), 0x20)) { revert(0, 0) }
            bal := mload(0x00)
        }
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

    /// @notice Kontrat'ın belirli bir token bakiyesini döndür (assembly optimized)
    function getBalance(address token) external view returns (uint256 bal) {
        assembly {
            mstore(0x00, 0x70a0823100000000000000000000000000000000000000000000000000000000)
            mstore(0x04, address())
            let ok := staticcall(gas(), token, 0x00, 0x24, 0x00, 0x20)
            // v22.1: returndatasize kontrolü eklendi — eksik/bozuk dönüş verisi koruması
            if or(iszero(ok), lt(returndatasize(), 0x20)) { revert(0, 0) }
            bal := mload(0x00)
        }
    }

    // ═════════════════════════════════════════════════════════════════════════
    //  INTERNAL — Güvenli ERC20 Transfer (Non-Standard Token Desteği)
    // ═════════════════════════════════════════════════════════════════════════

    /// @dev Assembly ERC20 transfer — abi.encodeWithSelector eliminasyonu.
    ///      USDT gibi bool dönmeyen token'ları da destekler.
    ///      ~200 gas tasarrufu (bellek genişleme + ABI encoding overhead eliminasyonu).
    function _safeTransfer(address token, address to, uint256 amt) internal {
        bool ok;
        assembly {
            // transfer(address,uint256) selector = 0xa9059cbb
            let ptr := mload(0x40) // free memory pointer
            mstore(ptr,        0xa9059cbb00000000000000000000000000000000000000000000000000000000)
            mstore(add(ptr, 4), to)
            mstore(add(ptr, 36), amt)
            ok := call(gas(), token, 0, ptr, 68, ptr, 32)
            // Non-standard token desteği: returndatasize 0 ise ok kabul et
            // Standart token: returndata 32 byte ve true olmalı
            if ok {
                switch returndatasize()
                case 0   { /* USDT-tarzı: veri dönmez → ok */ }
                case 32  { if iszero(mload(ptr)) { ok := 0 } }
                default  { ok := 0 }
            }
        }
        if (!ok) revert TransferFailed();
    }

    // ═════════════════════════════════════════════════════════════════════════
    //  RECEIVE — ETH kabul (WETH unwrap iadesi vb.)
    // ═════════════════════════════════════════════════════════════════════════

    receive() external payable {}
}
