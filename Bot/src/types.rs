// ============================================================================
//  TYPES — Paylaşılan Tipler, Yapılandırma ve İstatistikler
//  Arbitraj Botu v9.0 — Base Network
//
//  v9.0 Yenilikler:
//  ✓ Executor/Admin rol ayrımı (admin_address)
//  ✓ Deadline block desteği (deadline_blocks)
//  ✓ Dinamik bribe/priority fee modeli (bribe_pct)
//  ✓ Şifreli keystore desteği (keystore_path)
//  ✓ 134-byte calldata uyumu (deadlineBlock eklendi)
//
//  v7.0 (korunuyor):
//  ✓ NonceManager — AtomicU64 ile atomik nonce yönetimi
//  ✓ Token adresleri (weth_address, usdc_address) BotConfig'e eklendi
//  ✓ TickBitmap off-chain derinlik haritası yapıları
//  ✓ Multi-transport yapılandırması (IPC > WSS > HTTP)
// ============================================================================

use alloy::primitives::{address, Address, U256};
use eyre::Result;
use std::collections::{HashMap, HashSet};
use std::time::Instant;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use arc_swap::ArcSwap;

// ─────────────────────────────────────────────────────────────────────────────
// Token Whitelist — Güvenli Token Listesi (Base Network)
// ─────────────────────────────────────────────────────────────────────────────
//
// v10.1: Sadece yüksek likiditeli, kanıtlanmış tokenlar beyaz listede.
// Egzotik veya yeni çıkan tokenlar ile işlem yapılması engellenir.
// Bu, rüg-pull, düşük likidite kayası ve token manipulasyonu risklerini
// ortadan kaldırır.
//
// Desteklenen tokenlar:
//   • WETH  — Wrapped Ether (Base canonical)
//   • USDC  — USD Coin (Circle, bridged)
//   • USDT  — Tether USD (bridged)
//   • DAI   — Dai Stablecoin (bridged)
//   • cbETH — Coinbase Wrapped Staked ETH
// ─────────────────────────────────────────────────────────────────────────────

/// Base Network üzerindeki güvenli token adresleri (donanım kodlu whitelist)
pub fn token_whitelist() -> HashSet<Address> {
    HashSet::from([
        // WETH — Base canonical
        address!("4200000000000000000000000000000000000006"),
        // USDC — Circle (bridged)
        address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"),
        // USDbC — USD Base Coin (bridged via Base bridge)
        address!("d9aAEc86B65D86f6A7B5B1b0c42FFA531710b6CA"),
        // DAI — Dai Stablecoin (bridged)
        address!("50c5725949A6F0c72E6C4a641F24049A917DB0Cb"),
        // cbETH — Coinbase Wrapped Staked ETH
        address!("2Ae3F1Ec7F1F5012CFEab0185bfc7aa3cf0DEc22"),
        // cbBTC — Coinbase Wrapped BTC (8 decimals)
        address!("cbB7C0000aB88B473b1f5aFd9ef808440eed33Bf"),
        // AERO — Aerodrome Finance token
        address!("940181a94A35A4569E4529A3CDfB74e38FD98631"),
        // DEGEN — Degen token (18 decimals)
        address!("4ed4E862860beD51a9570b96d89aF5E1B0Efefed"),
    ])
}


/// uni_direction=0 → zeroForOne=true  → token0 input
/// uni_direction=1 → zeroForOne=false → token1 input
///
/// token0_is_weth=true:
///   - uni_dir=0 → token0(WETH) input → true
///   - uni_dir=1 → token1(USDC) input → false
///
/// token0_is_weth=false:
///   - uni_dir=0 → token0(USDC) input → false
///   - uni_dir=1 → token1(WETH) input → true
pub fn is_weth_input(uni_direction: u8, token0_is_weth: bool) -> bool {
    if uni_direction == 0 {
        // zeroForOne=true → token0 is input
        token0_is_weth
    } else {
        // oneForZero=true â†' token1 is input
        !token0_is_weth
    }
}

/// WETH miktarını hedef token miktarına çevir (human-readable → wei).
///
/// - Hedef WETH ise: amount_weth * 10^18
/// - Hedef quote token ise: amount_weth * eth_price_quote * 10^quote_decimals
///
/// Bu fonksiyon calldata'ya yazılacak amount değerini üretir.
pub fn weth_amount_to_input_wei(
    optimal_amount_weth: f64,
    is_weth_input: bool,
    eth_price_quote: f64,
    quote_token_decimals: u8,
) -> U256 {
    if is_weth_input {
        // Input WETH → 18 decimals
        U256::from(safe_f64_to_u128(optimal_amount_weth * 1e18))
    } else {
        // Input quote token → quote_token_decimals
        // WETH cinsinden miktar × ETH/Quote fiyatı × 10^decimals
        let scale = 10f64.powi(quote_token_decimals as i32);
        let quote_amount = optimal_amount_weth * eth_price_quote * scale;
        U256::from(safe_f64_to_u128(quote_amount))
    }
}

/// f64 → u128 güvenli dönüşüm (saturating).
///
/// NaN, Infinity, negatif veya u128::MAX üstü değerler için
/// Rust panic VERMEZ — yerine 0 veya u128::MAX döner.
/// MEV-kritik sistemlerde thread çökmesini önleyen savunma katmanı.
#[inline]
pub fn safe_f64_to_u128(val: f64) -> u128 {
    if val.is_nan() || val.is_infinite() || val < 0.0 {
        0
    } else if val >= u128::MAX as f64 {
        u128::MAX
    } else {
        // v22.0: Truncation → rounding. Wei cinsinden 0.5+ kaybı önler.
        // Ör: 1.9999 WETH → 1 WEI (truncation) vs 2 WEI (rounding)
        val.round() as u128
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DEX Türü
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DexType {
    UniswapV3,
    /// PancakeSwap V3 — slot0 feeProtocol alanı uint32 (Uniswap V3'te uint8)
    PancakeSwapV3,
    Aerodrome,
}

impl std::fmt::Display for DexType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DexType::UniswapV3 => write!(f, "Uniswap V3"),
            DexType::PancakeSwapV3 => write!(f, "PancakeSwap V3"),
            DexType::Aerodrome => write!(f, "Aerodrome"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Transport Modu (L2 Sequencer Optimizasyonu)
// ─────────────────────────────────────────────────────────────────────────────

/// Bağlantı transport tipi — Base L2 için IPC öncelikli
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportMode {
    /// IPC (Unix Domain Socket / Named Pipe) — En düşük gecikme (<0.1ms)
    Ipc,
    /// WebSocket — Orta gecikme (~1-5ms)
    Ws,
    /// HTTP — Yüksek gecikme (~5-50ms), fallback
    Http,
    /// Otomatik: IPC → WSS → HTTP sırasıyla dener
    Auto,
}

impl std::fmt::Display for TransportMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportMode::Ipc => write!(f, "IPC (Low Latency)"),
            TransportMode::Ws => write!(f, "WebSocket"),
            TransportMode::Http => write!(f, "HTTP"),
            TransportMode::Auto => write!(f, "Auto (IPC→WSS→HTTP)"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TickBitmap Yapıları (Off-Chain Derinlik Haritası)
// ─────────────────────────────────────────────────────────────────────────────

/// Tek bir başlatılmış tick'in bilgisi (Uniswap V3 ticks mapping)
///
/// Her tick sınırında likidite değişimi net olarak kaydedilir.
/// liquidityNet > 0 → o tick'e girildiğinde likidite ARTAR
/// liquidityNet < 0 → o tick'e girildiğinde likidite AZALIR
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct TickInfo {
    /// Toplam brüt likidite (pozisyon açma/kapama için)
    pub liquidity_gross: u128,
    /// Net likidite değişimi (tick geçişinde uygulanır)
    /// Pozitif: soldan sağa geçişte aktif likidite ARTAR
    /// Negatif: soldan sağa geçişte aktif likidite AZALIR
    pub liquidity_net: i128,
    /// Bu tick başlatılmış mı? (bitmap'te 1 ise true)
    pub initialized: bool,
}

/// Off-chain TickBitmap derinlik haritası
///
/// Zincirden çekilen iki veri kaynağını birleştirir:
///   1. tickBitmap(int16 wordPos) → uint256 : hangi tick'ler başlatılmış?
///   2. ticks(int24 tick) → TickInfo : başlatılmış tick'lerin detayları
///
/// Bu yapı, "50 ETH satarsam hangi 3 tick'i patlatırım?" sorusuna
/// mikrosaniye içinde cevap verir.
#[derive(Debug, Clone)]
pub struct TickBitmapData {
    /// Bitmap kelime haritası: wordPos → bitmap (256-bit)
    /// Her bit, tick_spacing'e göre belirli bir tick'in başlatılmış olup
    /// olmadığını gösterir.
    pub words: HashMap<i16, U256>,

    /// Başlatılmış tick'lerin detay bilgisi: tick → TickInfo
    /// Sadece initialized=true olan tick'ler burada bulunur.
    pub ticks: HashMap<i32, TickInfo>,

    /// Bu verinin okunduğu blok numarası
    pub snapshot_block: u64,

    /// Senkronizasyon süresi (mikrosaniye)
    pub sync_duration_us: u64,

    /// Taranan tick aralığı (current_tick ± range)
    pub scan_range: u32,
}

impl TickBitmapData {
    /// Boş bitmap oluştur
    pub fn empty() -> Self {
        Self {
            words: HashMap::new(),
            ticks: HashMap::new(),
            snapshot_block: 0,
            sync_duration_us: 0,
            scan_range: 0,
        }
    }

    /// Toplam başlatılmış tick sayısı
    pub fn initialized_tick_count(&self) -> usize {
        self.ticks.len()
    }

    /// Mint event'inden in-memory güncelleme.
    /// tickLower sınırında liquidityNet += amount, tickUpper'da -= amount.
    pub fn update_from_mint(&mut self, tick_lower: i32, tick_upper: i32, amount: u128, tick_spacing: i32) {
        self.update_tick_boundary(tick_lower, amount as i128, tick_spacing);
        self.update_tick_boundary(tick_upper, -(amount as i128), tick_spacing);
    }

    /// Burn event'inden in-memory güncelleme (Mint'in tersi).
    pub fn update_from_burn(&mut self, tick_lower: i32, tick_upper: i32, amount: u128, tick_spacing: i32) {
        self.update_tick_boundary(tick_lower, -(amount as i128), tick_spacing);
        self.update_tick_boundary(tick_upper, amount as i128, tick_spacing);
    }

    /// Tek bir tick sınırının likidite bilgisini güncelle.
    /// liquidityGross sıfıra düşerse tick uninitialized olur ve bitmap'ten temizlenir.
    fn update_tick_boundary(&mut self, tick: i32, liquidity_net_delta: i128, tick_spacing: i32) {
        let entry = self.ticks.entry(tick).or_insert(TickInfo {
            liquidity_gross: 0,
            liquidity_net: 0,
            initialized: false,
        });

        let abs_delta = liquidity_net_delta.unsigned_abs();
        if liquidity_net_delta > 0 {
            entry.liquidity_gross = entry.liquidity_gross.saturating_add(abs_delta);
        } else {
            entry.liquidity_gross = entry.liquidity_gross.saturating_sub(abs_delta);
        }
        entry.liquidity_net = entry.liquidity_net.saturating_add(liquidity_net_delta);

        let was_initialized = entry.initialized;
        let new_initialized = entry.liquidity_gross > 0;
        entry.initialized = new_initialized;

        // Bitmap bit'i sadece initialized durumu değiştiğinde flip edilir
        if was_initialized != new_initialized {
            self.flip_bitmap_bit(tick, tick_spacing);
        }

        // Artık initialized değilse tick'i hafızadan sil
        if !new_initialized {
            self.ticks.remove(&tick);
        }
    }

    /// Bitmap'te belirli bir tick'in bit'ini XOR ile flip et.
    fn flip_bitmap_bit(&mut self, tick: i32, tick_spacing: i32) {
        let compressed = tick / tick_spacing;
        let word_pos = (compressed >> 8) as i16;
        let bit_pos = (compressed & 0xFF) as u8;
        let mask = U256::from(1u64) << bit_pos;
        let word = self.words.entry(word_pos).or_insert(U256::ZERO);
        *word ^= mask;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Havuz Yapılandırması
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PoolConfig {
    pub address: Address,
    pub name: String,
    pub fee_bps: u32,
    pub fee_fraction: f64,
    pub token0_decimals: u8,
    pub token1_decimals: u8,
    pub dex: DexType,
    /// token0 WETH (veya base_token) mi? (Base: WETH < USDC adres sırasında → token0=WETH)
    pub token0_is_weth: bool,
    /// Tick aralığı (Uniswap V3 %0.05 = 10, Aerodrome değişken)
    pub tick_spacing: i32,
    /// Quote token adresi (çift bazlı — matched_pools.json'dan)
    pub quote_token_address: Address,
    /// Base (sol taraf) token adresi — WETH pair'lerinde WETH, non-WETH pair'lerinde ilgili token
    pub base_token_address: Address,
}

// ─────────────────────────────────────────────────────────────────────────────
// Havuz Anlık Durumu (RAM'de tutulur)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PoolState {
    /// sqrtPriceX96 (ham U256 değer)
    pub sqrt_price_x96: U256,
    /// sqrtPriceX96 float versiyonu (hızlı hesap için)
    pub sqrt_price_f64: f64,
    /// Mevcut tick
    pub tick: i32,
    /// Anlık likidite (u128)
    pub liquidity: u128,
    /// Likidite float versiyonu (hızlı hesap için)
    pub liquidity_f64: f64,
    /// WETH fiyatı quote token cinsinden — ör: 25.5 (cbBTC) veya 2500.0 (USDC)
    pub eth_price_usd: f64,
    /// Son güncellenen blok numarası
    pub last_block: u64,
    /// Son güncelleme zamanı (yerel)
    pub last_update: Instant,
    /// Havuz başlatıldı mı?
    pub is_initialized: bool,
    /// Havuz bytecode'u (REVM için önbellek)
    pub bytecode: Option<Vec<u8>>,
    /// Off-chain TickBitmap derinlik haritası
    /// "50 ETH satarsam hangi tick'leri patlatırım?" sorusunu yanıtlar
    pub tick_bitmap: Option<TickBitmapData>,
    /// Zincirden okunan canlı fee (basis points, ör: 500 = %0.05)
    /// None ise config'teki statik fee_bps kullanılır
    pub live_fee_bps: Option<u32>,
    /// v10.0: Stale Data Guard — sync başarısız olduğunda true olarak
    /// işaretlenir. is_stale=true olan havuzlarla arbitraj YAPILMAZ.
    /// Başarılı sync sonrası otomatik olarak false'a döner.
    pub is_stale: bool,
}

impl Default for PoolState {
    fn default() -> Self {
        Self {
            sqrt_price_x96: U256::ZERO,
            sqrt_price_f64: 0.0,
            tick: 0,
            liquidity: 0,
            liquidity_f64: 0.0,
            eth_price_usd: 0.0,
            last_block: 0,
            last_update: Instant::now(),
            is_initialized: false,
            bytecode: None,
            tick_bitmap: None,
            live_fee_bps: None,
            is_stale: false,
        }
    }
}

impl PoolState {
    /// Havuz aktif mi? (veriler geçerli mi?)
    /// v10.0: is_stale=true olan havuzlar artık aktif sayılmaz.
    pub fn is_active(&self) -> bool {
        self.is_initialized && !self.is_stale && self.eth_price_usd > 0.0 && self.liquidity > 0
    }

    /// Verinin yaşı (milisaniye)
    pub fn staleness_ms(&self) -> u128 {
        self.last_update.elapsed().as_millis()
    }

    /// v10.0: Veri taze mi? (aktif + staleness eşiğinin altında)
    /// Hard-abort kontrolü için kullanılır.
    pub fn is_fresh(&self, max_staleness_ms: u128) -> bool {
        self.is_active() && self.staleness_ms() <= max_staleness_ms
    }
}

/// Thread-safe havuz durumu (Lock-free: ArcSwap ile atomik pointer swap)
pub type SharedPoolState = Arc<ArcSwap<PoolState>>;

// ─────────────────────────────────────────────────────────────────────────────
// Dinamik Atomik Nonce Yöneticisi
// ─────────────────────────────────────────────────────────────────────────────

/// Lock-free, atomik nonce yöneticisi.
///
/// Problem: Her blokta `provider.get_transaction_count()` çağırmak sıralı
/// RPC gecikmesi yaratır ve yarış durumuna (race condition) açıktır.
///
/// Çözüm: Bot başlangıcında nonce RPC'den bir kez okunur, sonra her TX
/// gönderiminde atomik olarak artırılır:
///
/// ```text
/// Bot başlatılır → RPC: eth_getTransactionCount → nonce = 42
/// TX #1 gönder → nonce = 42, AtomicU64::fetch_add(1) → nonce = 43
/// TX #2 gönder → nonce = 43, AtomicU64::fetch_add(1) → nonce = 44
/// ```
///
/// Sıfır ek gecikme, sıfır kilit çekişmesi.
pub struct NonceManager {
    current_nonce: AtomicU64,
}

impl NonceManager {
    /// Başlangıç nonce değeriyle oluştur (RPC'den okunan değer)
    pub fn new(initial_nonce: u64) -> Self {
        Self {
            current_nonce: AtomicU64::new(initial_nonce),
        }
    }

    /// Mevcut nonce'u al ve atomik olarak 1 artır.
    /// Dönen değer: TX'e yazılacak nonce (artmadan önceki değer)
    pub fn get_and_increment(&self) -> u64 {
        self.current_nonce.fetch_add(1, Ordering::SeqCst)
    }

    /// Mevcut nonce'u oku (artırmadan)
    pub fn current(&self) -> u64 {
        self.current_nonce.load(Ordering::SeqCst)
    }


    /// Nonce'u belirli bir değere zorla ayarla (RPC senkronizasyonu için)
    pub fn force_set(&self, nonce: u64) {
        self.current_nonce.store(nonce, Ordering::SeqCst);
    }
}

impl std::fmt::Debug for NonceManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NonceManager(nonce={})", self.current())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Arbitraj Fırsatı
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ArbitrageOpportunity {
    /// Ucuz havuz indeksi (buradan al)
    pub buy_pool_idx: usize,
    /// Pahalı havuz indeksi (buraya sat)
    pub sell_pool_idx: usize,
    /// Newton-Raphson ile hesaplanan optimal WETH miktarı
    pub optimal_amount_weth: f64,
    /// Beklenen net kâr (WETH cinsinden)
    pub expected_profit_weth: f64,
    /// Alış fiyatı (ucuz havuz ETH/Quote)
    pub buy_price_quote: f64,
    /// Satış fiyatı (pahalı havuz ETH/Quote)
    pub sell_price_quote: f64,
    /// Spread yüzdesi
    pub spread_pct: f64,
    /// Newton-Raphson yakınsadı mı?
    pub nr_converged: bool,
    /// Newton-Raphson iterasyon sayısı
    pub nr_iterations: u32,
}

// ─────────────────────────────────────────────────────────────────────────────
// Multi-Hop Arbitraj Fırsatı (v29.0: Route Engine)
// ─────────────────────────────────────────────────────────────────────────────

/// Multi-hop arbitraj fırsatı (3+ havuzlu triangular/quad arbitraj)
#[derive(Debug, Clone)]
pub struct MultiHopOpportunity {
    /// Rota indeksi (route_engine tarafından üretilen rota listesinde)
    pub route_idx: usize,
    /// Rotadaki havuz indeksleri (sıralı)
    pub pool_indices: Vec<usize>,
    /// Her hop'un swap yönü
    pub directions: Vec<bool>,
    /// Newton-Raphson ile hesaplanan optimal WETH miktarı
    pub optimal_amount_weth: f64,
    /// Beklenen net kâr (WETH cinsinden)
    pub expected_profit_weth: f64,
    /// Rota açıklaması (log/debug)
    pub label: String,
    /// Newton-Raphson yakınsadı mı?
    pub nr_converged: bool,
    /// Newton-Raphson iterasyon sayısı
    pub nr_iterations: u32,
    /// Hop sayısı
    pub hop_count: usize,
}

// ─────────────────────────────────────────────────────────────────────────────
// REVM Simülasyon Sonucu
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SimulationResult {
    /// Simülasyon başarılı mı?
    pub success: bool,
    /// Kullanılan gas
    pub gas_used: u64,
    /// Hata mesajı (varsa)
    pub error: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Bot Yapılandırması (.env tabanlı)
// ─────────────────────────────────────────────────────────────────────────────

#[allow(dead_code)]
pub struct BotConfig {
    /// WebSocket RPC URL (blok başlığı aboneliği için)
    pub rpc_wss_url: String,
    /// HTTP RPC URL (durum okuma için — gelecekte kullanılabilir)
        pub rpc_http_url: String,
    /// IPC bağlantı yolu (Unix socket / Windows named pipe)
        pub rpc_ipc_path: Option<String>,
    /// Transport modu (IPC > WSS > HTTP)
    pub transport_mode: TransportMode,
    /// Private key (kontrat tetikleme için, opsiyonel)
    /// v9.0: KeyManager üzerinden yönetilir, ama geriye uyumluluk için saklanır
    pub private_key: Option<String>,
    /// Arbitraj kontrat adresi (opsiyonel)
    pub contract_address: Option<Address>,
    /// WETH token adresi (Base: 0x4200000000000000000000000000000000000006)
    /// v12.0: Hardcoded — .env'den okunmaz, Base ağında sabittir.
    pub weth_address: Address,
    /// Tahmini gas maliyeti fallback (WETH cinsinden)
    pub gas_cost_fallback_weth: f64,
    /// Flash loan ücreti (basis points)
    pub flash_loan_fee_bps: f64,
    /// Minimum net kâr eşiği (WETH cinsinden)
    pub min_net_profit_weth: f64,
    /// İstatistik gösterme aralığı (blok sayısı)
    pub stats_interval: u64,
    /// Maks yeniden bağlanma denemesi (0 = sınırsız)
    pub max_retries: u32,
    /// Başlangıç bekleme süresi (saniye) — v10.1: agresif reconnect ile kullanılmıyor
        pub initial_retry_delay_secs: u64,
    /// Maksimum bekleme süresi (saniye) — v10.1: agresif reconnect ile kullanılmıyor
        pub max_retry_delay_secs: u64,
    /// Veri tazelik eşiği (milisaniye)
    pub max_staleness_ms: u128,
    /// Maksimum flash loan boyutu (WETH)
    pub max_trade_size_weth: f64,
    /// Base zincir ID
    pub chain_id: u64,
    /// TickBitmap tarama yarıçapı (mevcut tick ± range)
    /// v26.0: 500 → 100. No profitable arb moves price >5%.
    /// Narrower range cuts RPC data by ~80% and reduces parse overhead.
    pub tick_bitmap_range: u32,
    /// TickBitmap'in kaç blok eskiyene kadar geçerli sayılacağı
    pub tick_bitmap_max_age_blocks: u64,
    /// Gölge Modu (Shadow Mode): false ise fırsatlar loglanır, TX gönderilmez
    /// .env'deki EXECUTION_ENABLED ile kontrol edilir
    pub execution_enabled_flag: bool,

    // ── v9.0: Yeni Güvenlik ve Performans Alanları ──────────────

    /// Admin adresi — fon çekme yetkisi (soğuk cüzdan / multisig)
    /// v9.0 kontrat: admin rolü. Boşsa executor adresi kullanılır.
        pub admin_address: Option<Address>,
    /// Deadline block offset — calldata'ya eklenir, kontrat kontrol eder
    /// Ör: 2 → mevcut blok + 2 = son geçerli blok
    pub deadline_blocks: u32,
    /// Dinamik bribe yüzdesi — beklenen kârın bu oranı builder'a verilir
    /// Ör: 0.25 = %25, coinbase.transfer veya yüksek priority fee olarak
    pub bribe_pct: f64,
    /// Şifreli keystore dosya yolu (v9.0 key management)
        pub keystore_path: Option<String>,
    /// Key Manager modu aktif mi? (auto_load tarafından ayarlanır)
    pub key_manager_active: bool,
    /// v10.1: Circuit breaker eşiği — kaç ardışık başarısızlıkta bot kapanır
    /// Varsayılan: 3. .env'den CIRCUIT_BREAKER_THRESHOLD ile ayarlanabilir.
    pub circuit_breaker_threshold: u32,
    /// v15.0: Yedek RPC WebSocket URL (failover için)
    /// Primary RPC'de hata veya yüksek gecikme olursa backup'a geçilir.
    pub rpc_wss_url_backup: Option<String>,
    /// v15.0: Gecikme spike uyarı eşiği (ms)
    /// Bu değerin üzerinde gecikme loglanır.
    pub latency_spike_threshold_ms: f64,
    /// v10.0: Private/Flashbots RPC URL (MEV koruması için)
    /// Tanımlıysa eth_sendRawTransaction kullanılır, yoksa işlem İPTAL EDİLİR
    pub private_rpc_url: Option<String>,
    /// v10.0: Ek WSS RPC URL'leri (Round-Robin havuz için)
    /// Primary + backup dışında 3. endpoint
    pub rpc_wss_url_extra: Vec<String>,
    /// v21.0: Maksimum havuz komisyon tavanı (basis points)
    /// Bu değerin üzerindeki fee'ye sahip havuzlar strateji değerlendirmesinde atlanır.
    /// Varsayılan: 5 bps (%0.05). .env'den MAX_POOL_FEE_BPS ile ayarlanabilir.
    pub max_pool_fee_bps: u32,
    /// Minimum TVL eşiği (USD) — keşif motorunda kullanılır
    pub min_tvl_usd: f64,
    /// Minimum 24 saatlik işlem hacmi eşiği (USD)
    pub min_volume_24h_usd: f64,
    /// Maksimum takip edilen havuz sayısı
    pub max_tracked_pools: usize,
}

/// Hard-limit fee tier sabiti (basis points). Bu değer üzerindeki havuzlar
/// takibe alınmaz. %0.05 = 5 bps.
pub const MAX_FEE_TIER_BPS: u32 = 5;

impl BotConfig {
    /// .env dosyasından yapılandırmayı oku
    pub fn from_env() -> Result<Self> {
        let rpc_wss_url = std::env::var("RPC_WSS_URL")
            .map_err(|_| eyre::eyre!("RPC_WSS_URL must be defined in .env!"))?;

        if rpc_wss_url.is_empty() || rpc_wss_url.starts_with("wss://your-") {
            return Err(eyre::eyre!("RPC_WSS_URL must be a valid URL!"));
        }

        // v15.0: Yedek RPC URL (opsiyonel)
        let rpc_wss_url_backup = std::env::var("RPC_WSS_URL_BACKUP")
            .ok()
            .filter(|u| !u.is_empty() && !u.starts_with("wss://your-"));

        let rpc_http_url = std::env::var("RPC_HTTP_URL")
            .map_err(|_| eyre::eyre!("RPC_HTTP_URL must be defined in .env!"))?;

        if rpc_http_url.is_empty() || rpc_http_url.starts_with("https://your-") {
            return Err(eyre::eyre!("RPC_HTTP_URL must be a valid URL!"));
        }

        let private_key = std::env::var("PRIVATE_KEY")
            .ok()
            .filter(|pk| !pk.is_empty() && pk != "your-private-key-here");

        let contract_address = std::env::var("ARBITRAGE_CONTRACT_ADDRESS")
            .ok()
            .filter(|addr| !addr.is_empty() && addr != "0xYourContractAddress")
            .and_then(|addr| addr.parse::<Address>().ok());

        // ── WETH Adresi (Base sabit) ─────────────────────────────
        // v12.0: Legacy env var'lar (WETH_ADDRESS, QUOTE_TOKEN_*,
        // WETH_IS_TOKEN0, TOKEN0_DECIMALS, TOKEN1_DECIMALS) görmezden geliniyor.
        // Havuz bazlı token bilgileri matched_pools.json'dan geliyor.
        let weth_address: Address = address!("4200000000000000000000000000000000000006");

        let gas_cost_fallback_weth = Self::parse_env_f64("GAS_COST_FALLBACK_WETH", 0.00005);
        let flash_loan_fee_bps = Self::parse_env_f64("FLASH_LOAN_FEE_BPS", 5.0);
        // v26.0: Default 0.001 → 0.00003 WETH (Base L2 micro-profit strategy)
        // L2 gas is ~$0.01, collect frequent micro profits instead of rare large ones
        let min_net_profit_weth = Self::parse_env_f64("MIN_NET_PROFIT_WETH", 0.00003);
        // v28.0: Default 50.0 → 5.0 WETH. Base L2 havuz derinlikleri genelde
        // 0.05-2 WETH aralığındadır. Bot effective_cap ile sınırlar ama yüksek
        // default NR tarama aralığını şişirir ve hesaplama süresi harcar.
        let max_trade_size_weth = Self::parse_env_f64("MAX_TRADE_SIZE_WETH", 5.0);

        let stats_interval = std::env::var("STATS_INTERVAL")
            .unwrap_or_else(|_| "10".into())
            .parse::<u64>()
            .unwrap_or(10);

        let max_retries = std::env::var("MAX_RETRIES")
            .unwrap_or_else(|_| "0".into())
            .parse::<u32>()
            .unwrap_or(0);

        // v28.0: Default 2000 → 3000ms (SYNC_TIMEOUT_MS ile uyumlu)
        let max_staleness_ms = std::env::var("MAX_STALENESS_MS")
            .unwrap_or_else(|_| "3000".into())
            .parse::<u128>()
            .unwrap_or(3000);

        let chain_id = std::env::var("CHAIN_ID")
            .unwrap_or_else(|_| "8453".into())
            .parse::<u64>()
            .unwrap_or(8453);

        // ── IPC ve Transport Ayarları ─────────────────────────────
        let rpc_ipc_path = std::env::var("RPC_IPC_PATH")
            .ok()
            .filter(|p| !p.is_empty());

        let transport_mode = match std::env::var("TRANSPORT_MODE")
            .unwrap_or_else(|_| "auto".into())
            .to_lowercase()
            .as_str()
        {
            "ipc" => TransportMode::Ipc,
            "ws" | "wss" | "websocket" => TransportMode::Ws,
            "http" | "https" => TransportMode::Http,
            _ => TransportMode::Auto,
        };

        // ── TickBitmap Ayarları ───────────────────────────────────
        // v26.0: Default 500 → 100. Arbitrage never moves price >5%.
        // Reduces RPC payload ~80%, cuts parsing overhead significantly.
        let tick_bitmap_range = std::env::var("TICK_BITMAP_RANGE")
            .unwrap_or_else(|_| "100".into())
            .parse::<u32>()
            .unwrap_or(100);

        let tick_bitmap_max_age_blocks = std::env::var("TICK_BITMAP_MAX_AGE_BLOCKS")
            .unwrap_or_else(|_| "5".into())
            .parse::<u64>()
            .unwrap_or(5);

        // ── Gölge Modu (Shadow Mode) ─────────────────────────────
        // EXECUTION_ENABLED=true → gerçek TX gönder
        // EXECUTION_ENABLED=false veya tanımsız → sadece logla
        let execution_enabled_flag = std::env::var("EXECUTION_ENABLED")
            .unwrap_or_else(|_| "false".into())
            .to_lowercase()
            .parse::<bool>()
            .unwrap_or(false);

        // ── v9.0: Yeni Güvenlik ve Performans Ayarları ───────────

        // Admin adresi (fon çekme yetkisi — kontrat v9.0)
        let admin_address = std::env::var("ADMIN_ADDRESS")
            .ok()
            .filter(|addr| !addr.is_empty())
            .and_then(|addr| addr.parse::<Address>().ok());

        // Deadline block offset (varsayılan: 2 blok)
        let deadline_blocks = std::env::var("DEADLINE_BLOCKS")
            .unwrap_or_else(|_| "2".into())
            .parse::<u32>()
            .unwrap_or(2);

        // Dinamik bribe yüzdesi (varsayılan: %25)
        let bribe_pct = Self::parse_env_f64("BRIBE_PCT", 0.25);

        // v10.1: Circuit breaker eşiği (varsayılan: 3)
        let circuit_breaker_threshold = std::env::var("CIRCUIT_BREAKER_THRESHOLD")
            .unwrap_or_else(|_| "3".into())
            .parse::<u32>()
            .unwrap_or(3);

        // Şifreli keystore dosya yolu
        let keystore_path = std::env::var("KEYSTORE_PATH")
            .ok()
            .filter(|p| !p.is_empty());

        Ok(Self {
            rpc_wss_url,
            rpc_http_url,
            rpc_ipc_path,
            transport_mode,
            private_key,
            contract_address,
            weth_address,
            gas_cost_fallback_weth,
            flash_loan_fee_bps,
            min_net_profit_weth,
            stats_interval,
            max_retries,
            initial_retry_delay_secs: 2,
            max_retry_delay_secs: 60,
            max_staleness_ms,
            max_trade_size_weth,
            chain_id,
            tick_bitmap_range,
            tick_bitmap_max_age_blocks,
            execution_enabled_flag,
            admin_address,
            deadline_blocks,
            bribe_pct,
            keystore_path,
            key_manager_active: false, // main.rs'de KeyManager başlatıldıktan sonra güncellenir
            circuit_breaker_threshold,
            rpc_wss_url_backup,
            latency_spike_threshold_ms: Self::parse_env_f64("LATENCY_SPIKE_THRESHOLD_MS", 200.0),
            private_rpc_url: std::env::var("PRIVATE_RPC_URL")
                .ok()
                .filter(|u| !u.is_empty()),
            rpc_wss_url_extra: {
                let mut extras = Vec::new();
                // RPC_WSS_URL_2, RPC_WSS_URL_3 opsiyonel ek endpoint'ler
                for key in &["RPC_WSS_URL_2", "RPC_WSS_URL_3"] {
                    if let Ok(url) = std::env::var(key) {
                        if !url.is_empty() && !url.starts_with("wss://your-") {
                            extras.push(url);
                        }
                    }
                }
                extras
            },
            max_pool_fee_bps: std::env::var("MAX_POOL_FEE_BPS")
                .unwrap_or_else(|_| MAX_FEE_TIER_BPS.to_string())
                .parse::<u32>()
                .unwrap_or(MAX_FEE_TIER_BPS),
            min_tvl_usd: Self::parse_env_f64("MIN_TVL_USD", 1_000_000.0),
            min_volume_24h_usd: Self::parse_env_f64("MIN_VOLUME_24H_USD", 500_000.0),
            max_tracked_pools: std::env::var("MAX_TRACKED_POOLS")
                .unwrap_or_else(|_| "4".into())
                .parse::<usize>()
                .unwrap_or(4),
        })
    }

    /// Kontrat tetikleme modu aktif mi?
    /// Koşullar:
    ///   1. EXECUTION_ENABLED=true (.env)
    ///   2. Private key mevcut (keystore VEYA env var)
    ///   3. ARBITRAGE_CONTRACT_ADDRESS tanımlı
    pub fn execution_enabled(&self) -> bool {
        self.execution_enabled_flag
            && (self.private_key.is_some() || self.key_manager_active)
            && self.contract_address.is_some()
    }

    /// Gölge modu aktif mi? (Loglama yapılır ama TX gönderilmez)
    pub fn shadow_mode(&self) -> bool {
        !self.execution_enabled_flag
    }

    /// .env'den f64 oku
    fn parse_env_f64(key: &str, default: f64) -> f64 {
        std::env::var(key)
            .unwrap_or_else(|_| default.to_string())
            .parse::<f64>()
            .unwrap_or(default)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// load_pool_configs_from_env() SİLİNDİ — v11.0
// Havuz yapılandırması artık matched_pools.json'dan pool_discovery::build_runtime()
// ile yüklenir. Statik POOL_A/B_ADDRESS env var'ları kullanılmaz.
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// Arbitraj İstatistikleri
// ─────────────────────────────────────────────────────────────────────────────

pub struct ArbitrageStats {
    pub total_blocks_processed: u64,
    pub total_opportunities: u64,
    pub profitable_opportunities: u64,
    pub executed_trades: u64,
    pub failed_simulations: u64,
    pub max_spread_pct: f64,
    pub max_profit_weth: f64,
    pub total_potential_profit: f64,
    pub session_start: Instant,
    /// Transport türü (aktif bağlantı)
    pub active_transport: String,
    /// Ortalama blok işleme gecikmesi (ms)
    pub avg_block_latency_ms: f64,
    /// Minimum blok işleme gecikmesi (ms)
    pub min_block_latency_ms: f64,
    /// Toplam tick bitmap senkronizasyon sayısı
    pub tick_bitmap_syncs: u64,
    /// v10.0: Ardışık başarısızlık sayacı (circuit breaker için)
    /// 3 ardışık simülasyon/TX başarısızlığında bot geçici olarak durur
    pub consecutive_failures: u32,
    /// v15.0: Maksimum blok işleme gecikmesi (ms)
    pub max_block_latency_ms: f64,
    /// v15.0: Gecikme spike sayısı (threshold üzerinde)
    pub latency_spikes: u64,
    /// v23.0 (Y-1): Gölge modunda simülasyon başarılı fırsat sayısı
    pub shadow_sim_success: u64,
    /// v23.0 (Y-1): Gölge modunda simülasyon başarısız fırsat sayısı
    pub shadow_sim_fail: u64,
    /// v23.0 (Y-1): Gölge modunda kümülatif potansiyel kâr (WETH)
    pub shadow_cumulative_profit: f64,
}

impl ArbitrageStats {
    pub fn new() -> Self {
        Self {
            total_blocks_processed: 0,
            total_opportunities: 0,
            profitable_opportunities: 0,
            executed_trades: 0,
            failed_simulations: 0,
            max_spread_pct: 0.0,
            max_profit_weth: 0.0,
            total_potential_profit: 0.0,
            session_start: Instant::now(),
            active_transport: String::from("Unknown"),
            avg_block_latency_ms: 0.0,
            min_block_latency_ms: f64::MAX,
            tick_bitmap_syncs: 0,
            consecutive_failures: 0,
            max_block_latency_ms: 0.0,
            latency_spikes: 0,
            shadow_sim_success: 0,
            shadow_sim_fail: 0,
            shadow_cumulative_profit: 0.0,
        }
    }

    /// Blok gecikme istatistiğini güncelle
    pub fn update_latency(&mut self, latency_ms: f64) {
        if self.total_blocks_processed == 0 {
            self.avg_block_latency_ms = latency_ms;
        } else {
            // Kayan ortalama
            let n = self.total_blocks_processed as f64;
            self.avg_block_latency_ms = (self.avg_block_latency_ms * n + latency_ms) / (n + 1.0);
        }
        if latency_ms < self.min_block_latency_ms {
            self.min_block_latency_ms = latency_ms;
        }
        if latency_ms > self.max_block_latency_ms {
            self.max_block_latency_ms = latency_ms;
        }
    }

    pub fn uptime_str(&self) -> String {
        let secs = self.session_start.elapsed().as_secs();
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        let s = secs % 60;
        format!("{:02}:{:02}:{:02}", h, m, s)
    }
}
