// ============================================================================
//  TYPES — Paylaşılan Tipler, Yapılandırma ve İstatistikler
//  Arbitraj Botu v7.0 — Base Network
//
//  v7.0 Yenilikler:
//  ✓ NonceManager — AtomicU64 ile atomik nonce yönetimi
//  ✓ Token adresleri (weth_address, usdc_address) BotConfig'e eklendi
//  ✓ TickBitmap off-chain derinlik haritası yapıları
//  ✓ Multi-transport yapılandırması (IPC > WSS > HTTP)
// ============================================================================

use alloy::primitives::{Address, U256};
use eyre::Result;
use std::collections::HashMap;
use std::time::Instant;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use parking_lot::RwLock;

// ─────────────────────────────────────────────────────────────────────────────
// DEX Türü
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DexType {
    UniswapV3,
    Aerodrome,
}

impl std::fmt::Display for DexType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DexType::UniswapV3 => write!(f, "Uniswap V3"),
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
            TransportMode::Ipc => write!(f, "IPC (Düşük Gecikme)"),
            TransportMode::Ws => write!(f, "WebSocket"),
            TransportMode::Http => write!(f, "HTTP"),
            TransportMode::Auto => write!(f, "Otomatik (IPC→WSS→HTTP)"),
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

#[allow(dead_code)]
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

    /// Verilen tick'ten sonraki (sağdaki) en yakın başlatılmış tick'i bul
    /// direction: true = sağa (artan tick), false = sola (azalan tick)
    pub fn next_initialized_tick(&self, current_tick: i32, direction_right: bool) -> Option<(i32, &TickInfo)> {
        if direction_right {
            self.ticks.iter()
                .filter(|(&t, info)| t > current_tick && info.initialized)
                .min_by_key(|(&t, _)| t)
                .map(|(&t, info)| (t, info))
        } else {
            self.ticks.iter()
                .filter(|(&t, info)| t <= current_tick && info.initialized)
                .max_by_key(|(&t, _)| t)
                .map(|(&t, info)| (t, info))
        }
    }

    /// Belirli bir aralıktaki tüm başlatılmış tick'leri sıralı döndür
    pub fn initialized_ticks_in_range(&self, from_tick: i32, to_tick: i32) -> Vec<(i32, TickInfo)> {
        let (lo, hi) = if from_tick <= to_tick {
            (from_tick, to_tick)
        } else {
            (to_tick, from_tick)
        };

        let mut result: Vec<(i32, TickInfo)> = self.ticks.iter()
            .filter(|(&t, info)| t >= lo && t <= hi && info.initialized)
            .map(|(&t, info)| (t, *info))
            .collect();

        result.sort_by_key(|(t, _)| *t);
        result
    }

    /// Bitmap verisi yeterince güncel mi?
    pub fn is_fresh(&self, current_block: u64, max_age_blocks: u64) -> bool {
        current_block.saturating_sub(self.snapshot_block) <= max_age_blocks
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
    /// token0 WETH mi? (Base: WETH < USDC adres sırasında → token0=WETH)
    pub token0_is_weth: bool,
    /// Tick aralığı (Uniswap V3 %0.05 = 10, Aerodrome değişken)
    pub tick_spacing: i32,
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
    /// ETH fiyatı (USDC cinsinden) — ör: 2500.45
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
        }
    }
}

impl PoolState {
    /// Havuz aktif mi? (veriler geçerli mi?)
    pub fn is_active(&self) -> bool {
        self.is_initialized && self.eth_price_usd > 0.0 && self.liquidity > 0
    }

    /// Verinin yaşı (milisaniye)
    pub fn staleness_ms(&self) -> u128 {
        self.last_update.elapsed().as_millis()
    }
}

/// Thread-safe havuz durumu
pub type SharedPoolState = Arc<RwLock<PoolState>>;

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

    /// TX başarısız olursa nonce'u geri al (decriment)
    pub fn rollback(&self) {
        self.current_nonce.fetch_sub(1, Ordering::SeqCst);
    }

    /// Nonce'u belirli bir değere zorla ayarla (RPC senkronizasyonu için)
    #[allow(dead_code)]
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
    /// Beklenen net kâr (USD)
    pub expected_profit_usd: f64,
    /// Alış fiyatı (ucuz havuz ETH/USDC)
    pub buy_price: f64,
    /// Satış fiyatı (pahalı havuz ETH/USDC)  
    pub sell_price: f64,
    /// Spread yüzdesi
    pub spread_pct: f64,
    /// Newton-Raphson yakınsadı mı?
    pub nr_converged: bool,
    /// Newton-Raphson iterasyon sayısı
    pub nr_iterations: u32,
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

pub struct BotConfig {
    /// WebSocket RPC URL (blok başlığı aboneliği için)
    pub rpc_wss_url: String,
    /// HTTP RPC URL (durum okuma için — gelecekte kullanılabilir)
    #[allow(dead_code)]
    pub rpc_http_url: String,
    /// IPC bağlantı yolu (Unix socket / Windows named pipe)
    pub rpc_ipc_path: Option<String>,
    /// Transport modu (IPC > WSS > HTTP)
    pub transport_mode: TransportMode,
    /// Private key (kontrat tetikleme için, opsiyonel)
    pub private_key: Option<String>,
    /// Arbitraj kontrat adresi (opsiyonel)
    pub contract_address: Option<Address>,
    /// WETH token adresi (Base: 0x4200000000000000000000000000000000000006)
    pub weth_address: Address,
    /// USDC token adresi (Base: 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913)
    pub usdc_address: Address,
    /// Tahmini gas maliyeti (USD)
    pub gas_cost_usd: f64,
    /// Flash loan ücreti (basis points)
    pub flash_loan_fee_bps: f64,
    /// Minimum net kâr eşiği (USD)
    pub min_net_profit_usd: f64,
    /// İstatistik gösterme aralığı (blok sayısı)
    pub stats_interval: u64,
    /// Maks yeniden bağlanma denemesi (0 = sınırsız)
    pub max_retries: u32,
    /// Başlangıç bekleme süresi (saniye)
    pub initial_retry_delay_secs: u64,
    /// Maksimum bekleme süresi (saniye)
    pub max_retry_delay_secs: u64,
    /// Veri tazelik eşiği (milisaniye)
    pub max_staleness_ms: u128,
    /// Maksimum flash loan boyutu (WETH)
    pub max_trade_size_weth: f64,
    /// Base zincir ID
    pub chain_id: u64,
    /// TickBitmap tarama yarıçapı (mevcut tick ± range)
    /// Varsayılan: 500 tick (Uniswap V3 %0.05 için ~5% fiyat aralığı)
    pub tick_bitmap_range: u32,
    /// TickBitmap'in kaç blok eskiyene kadar geçerli sayılacağı
    pub tick_bitmap_max_age_blocks: u64,
    /// Gölge Modu (Shadow Mode): false ise fırsatlar loglanır, TX gönderilmez
    /// .env'deki EXECUTION_ENABLED ile kontrol edilir
    pub execution_enabled_flag: bool,
}

impl BotConfig {
    /// .env dosyasından yapılandırmayı oku
    pub fn from_env() -> Result<Self> {
        let rpc_wss_url = std::env::var("RPC_WSS_URL")
            .map_err(|_| eyre::eyre!("RPC_WSS_URL .env dosyasında tanımlanmalıdır!"))?;

        if rpc_wss_url.is_empty() || rpc_wss_url.starts_with("wss://your-") {
            return Err(eyre::eyre!("RPC_WSS_URL geçerli bir URL olmalıdır!"));
        }

        let rpc_http_url = std::env::var("RPC_HTTP_URL")
            .map_err(|_| eyre::eyre!("RPC_HTTP_URL .env dosyasında tanımlanmalıdır!"))?;

        if rpc_http_url.is_empty() || rpc_http_url.starts_with("https://your-") {
            return Err(eyre::eyre!("RPC_HTTP_URL geçerli bir URL olmalıdır!"));
        }

        let private_key = std::env::var("PRIVATE_KEY")
            .ok()
            .filter(|pk| !pk.is_empty() && pk != "your-private-key-here");

        let contract_address = std::env::var("ARBITRAGE_CONTRACT_ADDRESS")
            .ok()
            .filter(|addr| !addr.is_empty() && addr != "0xYourContractAddress")
            .and_then(|addr| addr.parse::<Address>().ok());

        // ── Token Adresleri ───────────────────────────────────────
        let weth_address = std::env::var("WETH_ADDRESS")
            .unwrap_or_else(|_| "0x4200000000000000000000000000000000000006".into())
            .parse::<Address>()
            .unwrap_or_else(|_| "0x4200000000000000000000000000000000000006".parse::<Address>().unwrap());

        let usdc_address = std::env::var("USDC_ADDRESS")
            .unwrap_or_else(|_| "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".into())
            .parse::<Address>()
            .unwrap_or_else(|_| "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".parse::<Address>().unwrap());

        let gas_cost_usd = Self::parse_env_f64("GAS_COST_USD", 0.10);
        let flash_loan_fee_bps = Self::parse_env_f64("FLASH_LOAN_FEE_BPS", 5.0);
        let min_net_profit_usd = Self::parse_env_f64("MIN_NET_PROFIT_USD", 0.50);
        let max_trade_size_weth = Self::parse_env_f64("MAX_TRADE_SIZE_WETH", 50.0);

        let stats_interval = std::env::var("STATS_INTERVAL")
            .unwrap_or_else(|_| "10".into())
            .parse::<u64>()
            .unwrap_or(10);

        let max_retries = std::env::var("MAX_RETRIES")
            .unwrap_or_else(|_| "0".into())
            .parse::<u32>()
            .unwrap_or(0);

        let max_staleness_ms = std::env::var("MAX_STALENESS_MS")
            .unwrap_or_else(|_| "2000".into())
            .parse::<u128>()
            .unwrap_or(2000);

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
        let tick_bitmap_range = std::env::var("TICK_BITMAP_RANGE")
            .unwrap_or_else(|_| "500".into())
            .parse::<u32>()
            .unwrap_or(500);

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

        Ok(Self {
            rpc_wss_url,
            rpc_http_url,
            rpc_ipc_path,
            transport_mode,
            private_key,
            contract_address,
            weth_address,
            usdc_address,
            gas_cost_usd,
            flash_loan_fee_bps,
            min_net_profit_usd,
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
        })
    }

    /// Kontrat tetikleme modu aktif mi?
    /// Üç koşul:
    ///   1. EXECUTION_ENABLED=true (.env)
    ///   2. PRIVATE_KEY tanımlı
    ///   3. ARBITRAGE_CONTRACT_ADDRESS tanımlı
    pub fn execution_enabled(&self) -> bool {
        self.execution_enabled_flag
            && self.private_key.is_some()
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
// Havuz Adresleri (.env tabanlı)
// ─────────────────────────────────────────────────────────────────────────────

/// .env dosyasından havuz yapılandırmalarını oku
pub fn load_pool_configs_from_env() -> Result<Vec<PoolConfig>> {
    let pool_a_addr = std::env::var("POOL_A_ADDRESS")
        .map_err(|_| eyre::eyre!("POOL_A_ADDRESS .env dosyasında tanımlanmalıdır!"))?
        .parse::<Address>()
        .map_err(|e| eyre::eyre!("POOL_A_ADDRESS geçersiz adres: {}", e))?;

    let pool_a_name = std::env::var("POOL_A_NAME")
        .unwrap_or_else(|_| "Havuz A".into());

    let pool_a_fee_bps = std::env::var("POOL_A_FEE_BPS")
        .unwrap_or_else(|_| "5".into())
        .parse::<u32>()
        .unwrap_or(5);

    let pool_a_dex = match std::env::var("POOL_A_DEX")
        .unwrap_or_else(|_| "uniswap".into())
        .to_lowercase()
        .as_str()
    {
        "aerodrome" => DexType::Aerodrome,
        _ => DexType::UniswapV3,
    };

    let pool_b_addr = std::env::var("POOL_B_ADDRESS")
        .map_err(|_| eyre::eyre!("POOL_B_ADDRESS .env dosyasında tanımlanmalıdır!"))?
        .parse::<Address>()
        .map_err(|e| eyre::eyre!("POOL_B_ADDRESS geçersiz adres: {}", e))?;

    let pool_b_name = std::env::var("POOL_B_NAME")
        .unwrap_or_else(|_| "Havuz B".into());

    let pool_b_fee_bps = std::env::var("POOL_B_FEE_BPS")
        .unwrap_or_else(|_| "100".into())
        .parse::<u32>()
        .unwrap_or(100);

    let pool_b_dex = match std::env::var("POOL_B_DEX")
        .unwrap_or_else(|_| "aerodrome".into())
        .to_lowercase()
        .as_str()
    {
        "uniswap" => DexType::UniswapV3,
        _ => DexType::Aerodrome,
    };

    // Token sırası tespiti (Base Network: WETH=0x4200...0006 < USDC=0x8335...)
    // WETH_IS_TOKEN0=true → token0=WETH(18), token1=USDC(6)
    let weth_is_token0 = std::env::var("WETH_IS_TOKEN0")
        .unwrap_or_else(|_| "true".into())
        .to_lowercase()
        .parse::<bool>()
        .unwrap_or(true);

    // Decimal bilgileri: WETH=18, USDC=6 (token sırasına göre atanır)
    let (token0_decimals, token1_decimals) = if weth_is_token0 {
        (18u8, 6u8) // token0=WETH(18), token1=USDC(6)
    } else {
        (6u8, 18u8) // token0=USDC(6), token1=WETH(18)
    };

    // Tick spacing (.env'den oku, yoksa fee'ye göre varsayılan)
    let pool_a_tick_spacing = std::env::var("POOL_A_TICK_SPACING")
        .unwrap_or_else(|_| "10".into())
        .parse::<i32>()
        .unwrap_or(10);

    let pool_b_tick_spacing = std::env::var("POOL_B_TICK_SPACING")
        .unwrap_or_else(|_| "1".into())
        .parse::<i32>()
        .unwrap_or(1);

    Ok(vec![
        PoolConfig {
            address: pool_a_addr,
            name: pool_a_name,
            fee_bps: pool_a_fee_bps,
            fee_fraction: pool_a_fee_bps as f64 / 10_000.0,
            token0_decimals,
            token1_decimals,
            dex: pool_a_dex,
            token0_is_weth: weth_is_token0,
            tick_spacing: pool_a_tick_spacing,
        },
        PoolConfig {
            address: pool_b_addr,
            name: pool_b_name,
            fee_bps: pool_b_fee_bps,
            fee_fraction: pool_b_fee_bps as f64 / 10_000.0,
            token0_decimals,
            token1_decimals,
            dex: pool_b_dex,
            token0_is_weth: weth_is_token0,
            tick_spacing: pool_b_tick_spacing,
        },
    ])
}

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
    pub max_profit_usd: f64,
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
            max_profit_usd: 0.0,
            total_potential_profit: 0.0,
            session_start: Instant::now(),
            active_transport: String::from("Bilinmiyor"),
            avg_block_latency_ms: 0.0,
            min_block_latency_ms: f64::MAX,
            tick_bitmap_syncs: 0,
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
    }

    pub fn uptime_str(&self) -> String {
        let secs = self.session_start.elapsed().as_secs();
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        let s = secs % 60;
        format!("{:02}:{:02}:{:02}", h, m, s)
    }
}
