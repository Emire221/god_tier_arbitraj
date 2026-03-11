// ============================================================================
//  DISCOVERY ENGINE v1.0 — On-Chain & Real-Time Otonom Keşif Motoru
//
//  5 Ana Bileşen:
//  ✓ [Adım 1] Factory Listener — WSS log dinleyici (PoolCreated event)
//  ✓ [Adım 2] Multi-API Aggregator — DexScreener + GeckoTerminal fallback
//  ✓ [Adım 3] Hot-Reload — Çalışma zamanı sıcak güncelleme (RwLock)
//  ✓ [Adım 4] Garbage Collector — Soğuk havuz temizleme + dinamik odak
//  ✓ [Adım 5] Opportunity Scorer — Akıllı kâr potansiyeli puanlaması
// ============================================================================

use alloy::primitives::{address, Address, B256, U256};
use alloy::providers::{Provider, ProviderBuilder, WsConnect};
use alloy::rpc::types::Filter;
use eyre::Result;
use futures_util::StreamExt;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio_util::sync::CancellationToken;
use colored::*;

use crate::pool_discovery::PairCombo;
use crate::types::{
    token_whitelist, DexType, PoolConfig, PoolState, SharedPoolState,
};

// ─────────────────────────────────────────────────────────────────────────────
// Sabitler
// ─────────────────────────────────────────────────────────────────────────────

/// Uniswap V3 PoolCreated event topic0
/// keccak256("PoolCreated(address,address,uint24,int24,address)")
const POOL_CREATED_TOPIC_UNI_V3: [u8; 32] = hex_literal::hex!(
    "783cca1c0412dd0d695e784568c96da2e9c22ff989357a2e8b1d9b2b4e6b7118"
);

/// Aerodrome Slipstream PoolCreated event topic0
/// keccak256("PoolCreated(address,address,int24,address)")
const POOL_CREATED_TOPIC_AERO: [u8; 32] = hex_literal::hex!(
    "2128d88d14c80cb081c1252a5acff7a264671bf199ce226b53571a20c57c0c12"
);

/// Base Network DEX Factory Adresleri
const FACTORY_UNISWAP_V3: Address = address!("33128a8fC17869897dcE68Ed026d694621f6FDfD");
const FACTORY_AERODROME_CL: Address = address!("5e7BB104d84c7CB9B682AaC2F3d509f5F406809A");
const FACTORY_PANCAKESWAP_V3: Address = address!("0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865");
const FACTORY_SUSHISWAP_V3: Address = address!("c35DADB65012eC5796536bD9864eD8773aBc74C4");

/// GeckoTerminal API base URL
const GECKO_TERMINAL_API: &str = "https://api.geckoterminal.com/api/v2";

/// WETH adresi (Base)
#[allow(dead_code)]
const BASE_WETH: Address = address!("4200000000000000000000000000000000000006");

/// Varsayılan yapılandırma değerleri
const DEFAULT_MAX_ACTIVE_POOLS: usize = 50;
const DEFAULT_COOLDOWN_BLOCKS: u64 = 500;
const DEFAULT_SCORE_INTERVAL_BLOCKS: u64 = 300; // ~10 dakika (Base ~2s blok)
const DEFAULT_API_POLL_INTERVAL_SECS: u64 = 300; // 5 dakika
const DEFAULT_GC_INTERVAL_BLOCKS: u64 = 150;     // ~5 dakika

// ─────────────────────────────────────────────────────────────────────────────
// Keşif Kaynağı
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum DiscoverySource {
    FactoryEvent,
    DexScreener,
    GeckoTerminal,
    Manual,
}

impl std::fmt::Display for DiscoverySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FactoryEvent => write!(f, "On-Chain Factory"),
            Self::DexScreener => write!(f, "DexScreener API"),
            Self::GeckoTerminal => write!(f, "GeckoTerminal API"),
            Self::Manual => write!(f, "Manuel"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Keşif Yapılandırması
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    /// Maksimum aktif izleme havuzu sayısı
    pub max_active_pools: usize,
    /// Hareketsiz havuz cool-down eşiği (blok sayısı)
    pub cooldown_blocks: u64,
    /// Skor güncelleme aralığı (blok sayısı)
    pub score_interval_blocks: u64,
    /// API yoklama aralığı (saniye)
    pub api_poll_interval_secs: u64,
    /// Çöp toplayıcı aralığı (blok sayısı)
    pub gc_interval_blocks: u64,
    /// Minimum likidite eşiği (USD)
    pub min_liquidity_usd: f64,
    /// Minimum 24s hacim eşiği (USD)
    pub min_volume_24h_usd: f64,
    /// Maksimum havuz komisyonu (basis points)
    pub max_fee_bps: u32,
    /// WSS RPC URL (factory listener için)
    pub wss_url: String,
    /// WETH adresi
    pub weth_address: Address,
}

impl DiscoveryConfig {
    /// BotConfig'ten discovery yapılandırması oluştur
    pub fn from_bot_config(
        wss_url: &str,
        max_fee_bps: u32,
        weth_address: Address,
    ) -> Self {
        Self {
            max_active_pools: DEFAULT_MAX_ACTIVE_POOLS,
            cooldown_blocks: DEFAULT_COOLDOWN_BLOCKS,
            score_interval_blocks: DEFAULT_SCORE_INTERVAL_BLOCKS,
            api_poll_interval_secs: DEFAULT_API_POLL_INTERVAL_SECS,
            gc_interval_blocks: DEFAULT_GC_INTERVAL_BLOCKS,
            min_liquidity_usd: 50_000.0,
            min_volume_24h_usd: 10_000.0,
            max_fee_bps,
            wss_url: wss_url.to_string(),
            weth_address,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Bekleyen Havuz (Pending Pool) — Keşfedilmiş ama henüz aktif değil
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PendingPool {
    pub config: PoolConfig,
    pub source: DiscoverySource,
    pub discovered_at: Instant,
    pub score: f64,
}

// ─────────────────────────────────────────────────────────────────────────────
// Havuz Aktivite İzleme (Garbage Collector için)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PoolActivity {
    /// Son swap gözlemlenen blok
    pub last_swap_block: u64,
    /// Son N blok içindeki swap sayısı
    pub swap_count: u32,
    /// Kümülatif spread (ortalama hesabı için)
    pub cumulative_spread: f64,
    /// Spread ölçüm sayısı
    pub spread_samples: u32,
    /// Son 1 saatlik tahmini hacim (USD)
    pub estimated_volume_1h: f64,
    /// Skora dahil edilme zamanı
    pub last_score_update: Instant,
}

impl Default for PoolActivity {
    fn default() -> Self {
        Self {
            last_swap_block: 0,
            swap_count: 0,
            cumulative_spread: 0.0,
            spread_samples: 0,
            estimated_volume_1h: 0.0,
            last_score_update: Instant::now(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Havuz Skoru (Opportunity Score)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PoolScore {
    /// Genel fırsat skoru (yüksek = daha iyi)
    pub score: f64,
    /// Son 1 saatlik hacim (USD)
    pub volume_1h: f64,
    /// Tahmini spread dalgalanması
    pub spread_volatility: f64,
    /// Havuz komisyonu (fraction, ör: 0.0005)
    pub fee_fraction: f64,
    /// Son güncelleme anı
    pub updated_at: Instant,
}

impl PoolScore {
    /// Fırsat Skoru = (Hacim × Spread Dalgalanması) / Komisyon
    pub fn calculate(volume_1h: f64, spread_volatility: f64, fee_fraction: f64) -> f64 {
        if fee_fraction <= 0.0 {
            return 0.0;
        }
        (volume_1h * spread_volatility) / fee_fraction
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// LIVE POOL REGISTRY — Paylaşımlı Canlı Havuz Kayıt Defteri
// ─────────────────────────────────────────────────────────────────────────────

pub struct LivePoolRegistry {
    /// Bekleyen ekleme kuyruğu (keşif motoru tarafından doldurulur)
    pending_additions: Vec<PendingPool>,
    /// Havuz aktivite izleme (indeks bazlı)
    activity: HashMap<usize, PoolActivity>,
    /// Havuz skorları (indeks bazlı)
    scores: HashMap<usize, PoolScore>,
    /// Uykudaki havuzlar (aktif izlemeden çıkarılmış)
    sleeping_pools: Vec<PoolConfig>,
    /// Aktif havuz indeksleri (false = uyuyor/atla)
    active_flags: Vec<bool>,
    /// Bilinen havuz adresleri (tekrar eklemeyi önle)
    known_addresses: std::collections::HashSet<Address>,
    /// Son çöp toplama bloğu
    last_gc_block: u64,
    /// Son skor güncelleme bloğu
    last_score_block: u64,
    /// İstatistikler
    pub stats: RegistryStats,
}

#[derive(Debug, Clone, Default)]
pub struct RegistryStats {
    pub total_discovered: u64,
    pub factory_events_received: u64,
    pub api_discoveries: u64,
    pub pools_activated: u64,
    pub pools_garbage_collected: u64,
    pub score_recalculations: u64,
}

impl LivePoolRegistry {
    /// Mevcut havuz listesinden registry oluştur
    pub fn new(pools: &[PoolConfig]) -> Self {
        let mut known = std::collections::HashSet::new();
        for pool in pools {
            known.insert(pool.address);
        }
        Self {
            pending_additions: Vec::new(),
            activity: HashMap::new(),
            scores: HashMap::new(),
            sleeping_pools: Vec::new(),
            active_flags: vec![true; pools.len()],
            known_addresses: known,
            last_gc_block: 0,
            last_score_block: 0,
            stats: RegistryStats::default(),
        }
    }

    /// Keşif motorundan gelen yeni havuzu bekleyen kuyruğa ekle
    pub fn enqueue_pending(&mut self, pool: PendingPool) {
        // Tekrar eklemeyi önle
        if self.known_addresses.contains(&pool.config.address) {
            return;
        }
        self.known_addresses.insert(pool.config.address);
        self.stats.total_discovered += 1;
        self.pending_additions.push(pool);
    }

    /// Bekleyen havuzları çek (ana döngü tarafından çağrılır)
    pub fn take_pending(&mut self) -> Vec<PendingPool> {
        std::mem::take(&mut self.pending_additions)
    }

    /// Havuz aktivitesini güncelle (swap gözlemlendiğinde)
    #[allow(dead_code)]
    pub fn record_swap(&mut self, pool_idx: usize, block_number: u64) {
        let activity = self.activity.entry(pool_idx).or_default();
        activity.last_swap_block = block_number;
        activity.swap_count += 1;
    }

    /// Spread gözlemini kaydet (skor hesabı için)
    pub fn record_spread(&mut self, pool_idx: usize, spread_pct: f64) {
        let activity = self.activity.entry(pool_idx).or_default();
        activity.cumulative_spread += spread_pct;
        activity.spread_samples += 1;
    }

    /// Hacim bilgisini güncelle (API'den)
    #[allow(dead_code)]
    pub fn update_volume(&mut self, pool_idx: usize, volume_1h: f64) {
        let activity = self.activity.entry(pool_idx).or_default();
        activity.estimated_volume_1h = volume_1h;
    }

    /// Havuzun aktif olup olmadığını kontrol et
    pub fn is_active(&self, pool_idx: usize) -> bool {
        self.active_flags.get(pool_idx).copied().unwrap_or(false)
    }

    /// Soğuk havuzları tespit et ve uyku listesine al
    pub fn garbage_collect(
        &mut self,
        pools: &[PoolConfig],
        current_block: u64,
        cooldown_blocks: u64,
        max_active: usize,
    ) -> Vec<usize> {
        let mut deactivated = Vec::new();

        for (idx, active) in self.active_flags.iter_mut().enumerate() {
            if !*active {
                continue;
            }

            let should_deactivate = if let Some(activity) = self.activity.get(&idx) {
                // Son N blok boyunca swap gözlemlenmemiş
                let blocks_since_swap = current_block.saturating_sub(activity.last_swap_block);
                let no_swaps = blocks_since_swap > cooldown_blocks;

                // Spread sıfıra yakın (fırsat yok)
                let avg_spread = if activity.spread_samples > 0 {
                    activity.cumulative_spread / activity.spread_samples as f64
                } else {
                    0.0
                };
                let low_spread = avg_spread < 0.001;

                no_swaps && low_spread
            } else {
                // Hiç aktivite kaydı yok — soğuk
                current_block.saturating_sub(0) > cooldown_blocks
            };

            if should_deactivate {
                *active = false;
                if idx < pools.len() {
                    self.sleeping_pools.push(pools[idx].clone());
                }
                deactivated.push(idx);
                self.stats.pools_garbage_collected += 1;
            }
        }

        // Aktif havuz sayısını kontrol et (taşma uyarısı)
        let active_count = self.active_flags.iter().filter(|&&a| a).count();
        if active_count > max_active {
            eprintln!(
                "  {} [Registry] Aktif havuz sayısı ({}) maksimum limitin ({}) üstünde",
                "⚠️".yellow(), active_count, max_active,
            );
        }

        deactivated
    }

    /// Tüm aktif havuzların skorlarını yeniden hesapla
    pub fn recalculate_scores(
        &mut self,
        pools: &[PoolConfig],
        current_block: u64,
    ) {
        for (idx, pool) in pools.iter().enumerate() {
            if !self.is_active(idx) {
                continue;
            }

            let activity = self.activity.entry(idx).or_default();
            let volume = activity.estimated_volume_1h;
            let spread_vol = if activity.spread_samples > 0 {
                activity.cumulative_spread / activity.spread_samples as f64
            } else {
                0.0
            };

            let score = PoolScore::calculate(volume, spread_vol, pool.fee_fraction);

            self.scores.insert(idx, PoolScore {
                score,
                volume_1h: volume,
                spread_volatility: spread_vol,
                fee_fraction: pool.fee_fraction,
                updated_at: Instant::now(),
            });
        }

        self.last_score_block = current_block;
        self.stats.score_recalculations += 1;

        // Spread sayaçlarını sıfırla (yeni periyod için)
        for activity in self.activity.values_mut() {
            activity.cumulative_spread = 0.0;
            activity.spread_samples = 0;
        }
    }

    /// En yüksek skorlu N havuzun indekslerini döndür
    pub fn top_scored_indices(&self, n: usize) -> Vec<usize> {
        let mut scored: Vec<(usize, f64)> = self.scores.iter()
            .filter(|(&idx, _)| self.is_active(idx))
            .map(|(&idx, s)| (idx, s.score))
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.into_iter().take(n).map(|(idx, _)| idx).collect()
    }

    /// Uyuyan havuzları puanlarına göre yeniden aktive et (sıcak havuz yer açtığında)
    #[allow(dead_code)]
    pub fn reactivate_best_sleeping(
        &mut self,
        pools: &mut Vec<PoolConfig>,
        states: &mut Vec<SharedPoolState>,
        pair_combos: &mut Vec<PairCombo>,
        count: usize,
    ) -> usize {
        let to_wake = self.sleeping_pools.drain(..count.min(self.sleeping_pools.len())).collect::<Vec<_>>();
        let mut woken = 0;

        for pool in to_wake {
            if !self.known_addresses.contains(&pool.address) {
                continue;
            }
            let new_idx = pools.len();

            // v26.0: Reactivated havuzlar için pair combo üret
            // (aynı quote token'a sahip mevcut havuzlarla arbitraj çifti)
            for (existing_idx, existing_pool) in pools.iter().enumerate() {
                if existing_pool.quote_token_address == pool.quote_token_address
                    && existing_pool.address != pool.address
                {
                    let pair_name = format!("WETH/{:.8}", format!("{}", pool.quote_token_address));
                    pair_combos.push(PairCombo {
                        pair_name,
                        pool_a_idx: existing_idx,
                        pool_b_idx: new_idx,
                    });
                }
            }

            pools.push(pool);
            states.push(Arc::new(RwLock::new(PoolState::default())));
            self.active_flags.push(true);
            woken += 1;
        }

        woken
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DISCOVERY ENGINE — Tüm Bileşenlerin Orkestratörü
// ─────────────────────────────────────────────────────────────────────────────

pub struct DiscoveryEngine {
    registry: Arc<RwLock<LivePoolRegistry>>,
    config: DiscoveryConfig,
}

impl DiscoveryEngine {
    pub fn new(registry: Arc<RwLock<LivePoolRegistry>>, config: DiscoveryConfig) -> Self {
        Self { registry, config }
    }

    /// Tüm arka plan görevlerini başlat
    pub fn start(&self, cancel_token: CancellationToken) {
        // ── Görev 1: On-Chain Factory Listener ──
        {
            let registry = self.registry.clone();
            let config = self.config.clone();
            let token = cancel_token.clone();

            tokio::spawn(async move {
                tokio::select! {
                    _ = token.cancelled() => {
                        eprintln!("  {} Factory listener graceful shutdown", "🔌");
                    }
                    result = factory_listener(registry, &config) => {
                        match result {
                            Ok(_) => {}
                            Err(e) => {
                                eprintln!(
                                    "  {} Factory listener hatası (API keşfi devam ediyor): {}",
                                    "⚠️", e
                                );
                            }
                        }
                    }
                }
            });
        }

        // ── Görev 2: Multi-API Aggregator ──
        {
            let registry = self.registry.clone();
            let config = self.config.clone();
            let token = cancel_token.clone();

            tokio::spawn(async move {
                tokio::select! {
                    _ = token.cancelled() => {
                        eprintln!("  {} API aggregator graceful shutdown", "🔌");
                    }
                    _ = api_aggregator_loop(registry, &config) => {}
                }
            });
        }

        println!(
            "  {} Keşif Motoru v1.0 başlatıldı: Factory WSS + Multi-API + Skorlama + GC",
            "🔍".cyan()
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// [ADIM 1] ON-CHAIN FACTORY LISTENER — WebSocket PoolCreated Dinleyici
// ─────────────────────────────────────────────────────────────────────────────

/// Bilinen DEX factory kontratlarından PoolCreated eventlerini dinler.
/// Yeni havuz yaratıldığı milisaniyede yakalar ve registry'ye ekler.
async fn factory_listener(
    registry: Arc<RwLock<LivePoolRegistry>>,
    config: &DiscoveryConfig,
) -> Result<()> {
    let ws = WsConnect::new(&config.wss_url);
    let provider = ProviderBuilder::new().on_ws(ws).await
        .map_err(|e| eyre::eyre!("Factory listener WSS bağlantı hatası: {}", e))?;

    // Dinlenecek factory adresleri
    let factories = vec![
        FACTORY_UNISWAP_V3,
        FACTORY_AERODROME_CL,
        FACTORY_PANCAKESWAP_V3,
        FACTORY_SUSHISWAP_V3,
    ];

    // Her iki event topic'ini dinle (Uniswap V3 + Aerodrome)
    let topic_uni = B256::from(POOL_CREATED_TOPIC_UNI_V3);
    let topic_aero = B256::from(POOL_CREATED_TOPIC_AERO);

    let filter = Filter::new()
        .address(factories)
        .event_signature(vec![topic_uni, topic_aero]);

    let sub = provider.subscribe_logs(&filter).await
        .map_err(|e| eyre::eyre!("Factory event abonelik hatası: {}", e))?;
    let mut stream = sub.into_stream();

    println!(
        "  {} On-Chain Factory Listener aktif — 4 DEX factory dinleniyor",
        "🏭".green()
    );

    let whitelist = token_whitelist();

    while let Some(log) = stream.next().await {
        let topics = log.topics();
        if topics.is_empty() {
            continue;
        }

        let factory_address = log.inner.address;
        let topic0 = topics[0];

        // Event tipine göre parsing
        let parsed = if topic0 == topic_uni {
            parse_pool_created_uni_v3(&log, factory_address)
        } else if topic0 == topic_aero {
            parse_pool_created_aerodrome(&log, factory_address)
        } else {
            continue;
        };

        let parsed = match parsed {
            Some(p) => p,
            None => continue,
        };

        // Token whitelist kontrolü — sadece güvenli tokenlarla işlem
        if !whitelist.contains(&parsed.token0) || !whitelist.contains(&parsed.token1) {
            continue;
        }

        // WETH içeren çiftleri filtrele (arbitraj bot WETH bazlı çalışıyor)
        let has_weth = parsed.token0 == config.weth_address
            || parsed.token1 == config.weth_address;
        if !has_weth {
            continue;
        }

        // Fee filtresi
        let fee_bps = parsed.fee_bps;
        if fee_bps > config.max_fee_bps {
            continue;
        }

        // PoolConfig oluştur
        let (token0_is_weth, quote_addr) = if parsed.token0 == config.weth_address {
            (true, parsed.token1)
        } else {
            (false, parsed.token0)
        };

        let base_addr_resolved = if token0_is_weth { parsed.token0 } else { parsed.token1 };
        let pool_config = PoolConfig {
            address: parsed.pool_address,
            name: format!("{}-WETH/{}",
                match parsed.dex_type {
                    DexType::UniswapV3 => "UniV3",
                    DexType::PancakeSwapV3 => "PCS",
                    DexType::Aerodrome => "Aero",
                },
                format!("{:.6}...", format!("{}", quote_addr))
            ),
            fee_bps,
            fee_fraction: fee_bps as f64 / 10_000.0,
            token0_decimals: if token0_is_weth { 18 } else { infer_decimals(&parsed.token0) },
            token1_decimals: if token0_is_weth { infer_decimals(&parsed.token1) } else { 18 },
            dex: parsed.dex_type,
            token0_is_weth,
            tick_spacing: parsed.tick_spacing,
            quote_token_address: quote_addr,
            base_token_address: base_addr_resolved,
        };

        eprintln!(
            "  {} [Factory] Yeni havuz tespit edildi: {} ({}) — Fee: {}bps | Adres: {}",
            "🏭".green(),
            pool_config.name, parsed.dex_type,
            fee_bps, parsed.pool_address,
        );

        // Registry'ye ekle
        let mut reg = registry.write();
        reg.enqueue_pending(PendingPool {
            config: pool_config,
            source: DiscoverySource::FactoryEvent,
            discovered_at: Instant::now(),
            score: 0.0, // İlk skor — henüz veri yok
        });
        reg.stats.factory_events_received += 1;
    }

    Err(eyre::eyre!("Factory event stream kapandı"))
}

/// Uniswap V3 PoolCreated event parsing
/// topics: [hash, token0, token1, fee]
/// data: [tickSpacing(int24), pool(address)]
fn parse_pool_created_uni_v3(
    log: &alloy::rpc::types::Log,
    factory: Address,
) -> Option<ParsedPoolCreated> {
    let topics = log.topics();
    if topics.len() < 4 {
        return None;
    }

    let token0 = Address::from_word(topics[1]);
    let token1 = Address::from_word(topics[2]);
    let fee_raw = U256::from_be_bytes(topics[3].0);
    let fee_bps = (fee_raw.to::<u64>() / 100) as u32; // fee → bps (Uni V3 fee = basis points * 100)

    // data: tickSpacing (int24, left-padded to 32 bytes) + pool (address, left-padded)
    let data: &[u8] = log.inner.data.data.as_ref();
    if data.len() < 64 {
        return None;
    }

    // tickSpacing: bytes [0..32] → int24 (last 3 bytes, signed)
    let tick_spacing_bytes = &data[29..32];
    let tick_spacing = {
        let mut buf = [0u8; 4];
        // Sign extend from 3 bytes
        if tick_spacing_bytes[0] & 0x80 != 0 {
            buf[0] = 0xFF;
        }
        buf[1..4].copy_from_slice(tick_spacing_bytes);
        i32::from_be_bytes(buf)
    };

    // pool address: bytes [32..64] → last 20 bytes
    let pool_address = Address::from_slice(&data[44..64]);

    let dex_type = factory_to_dex_type(factory);

    Some(ParsedPoolCreated {
        token0,
        token1,
        fee_bps,
        tick_spacing,
        pool_address,
        dex_type,
    })
}

/// Aerodrome Slipstream PoolCreated event parsing
/// topics: [hash, token0, token1, tickSpacing]
/// data: [pool(address)]
fn parse_pool_created_aerodrome(
    log: &alloy::rpc::types::Log,
    _factory: Address,
) -> Option<ParsedPoolCreated> {
    let topics = log.topics();
    if topics.len() < 4 {
        return None;
    }

    let token0 = Address::from_word(topics[1]);
    let token1 = Address::from_word(topics[2]);

    // tickSpacing from topic[3]
    let tick_spacing_raw = topics[3].0;
    let tick_spacing = {
        let mut buf = [0u8; 4];
        buf.copy_from_slice(&tick_spacing_raw[28..32]);
        i32::from_be_bytes(buf)
    };

    // pool address from data
    let data: &[u8] = log.inner.data.data.as_ref();
    if data.len() < 32 {
        return None;
    }
    let pool_address = Address::from_slice(&data[12..32]);

    // Aerodrome fee: tick_spacing'e göre tahmin
    let fee_bps = match tick_spacing.abs() {
        1 => 1,
        10 | 50 => 5,
        100 => 30,
        200 => 100,
        _ => 5,
    };

    Some(ParsedPoolCreated {
        token0,
        token1,
        fee_bps,
        tick_spacing,
        pool_address,
        dex_type: DexType::Aerodrome,
    })
}

struct ParsedPoolCreated {
    token0: Address,
    token1: Address,
    fee_bps: u32,
    tick_spacing: i32,
    pool_address: Address,
    dex_type: DexType,
}

/// Factory adresi → DexType eşleştirme
fn factory_to_dex_type(factory: Address) -> DexType {
    if factory == FACTORY_AERODROME_CL {
        DexType::Aerodrome
    } else if factory == FACTORY_PANCAKESWAP_V3 {
        DexType::PancakeSwapV3
    } else {
        DexType::UniswapV3
    }
}

/// Token adresi → decimal tahmin
fn infer_decimals(token: &Address) -> u8 {
    let lower = format!("{}", token).to_lowercase();
    if lower.ends_with("0000000000000000000006") { 18 }
    else if lower.contains("cbb7c0000ab88b473b1f5afd9ef808440eed33bf") { 8 }
    else if lower.contains("833589fcd6edb6e08f4c7c32d4f71b54bda02913") { 6 }
    else if lower.contains("d9aaec86b65d86f6a7b5b1b0c42ffa531710b6ca") { 6 }
    else if lower.contains("50c5725949a6f0c72e6c4a641f24049a917db0cb") { 18 }
    else if lower.contains("2ae3f1ec7f1f5012cfeab0185bfc7aa3cf0dec22") { 18 }
    else { 18 }
}

// ─────────────────────────────────────────────────────────────────────────────
// [ADIM 2] MULTI-API AGGREGATOR — DexScreener + GeckoTerminal
// ─────────────────────────────────────────────────────────────────────────────

/// Periyodik API yoklama döngüsü — birden fazla kaynaktan havuz keşfi
async fn api_aggregator_loop(
    registry: Arc<RwLock<LivePoolRegistry>>,
    config: &DiscoveryConfig,
) {
    let interval = std::time::Duration::from_secs(config.api_poll_interval_secs);

    // İlk yoklamayı 30s geciktir (başlangıç senkronizasyonu bitmeden yarışma olmasın)
    tokio::time::sleep(std::time::Duration::from_secs(30)).await;

    loop {
        // Kaynak 1: DexScreener (birincil)
        match discover_dexscreener(config).await {
            Ok(pools) => {
                let count = pools.len();
                let mut reg = registry.write();
                for pool in pools {
                    reg.enqueue_pending(pool);
                }
                if count > 0 {
                    reg.stats.api_discoveries += count as u64;
                    eprintln!(
                        "  {} [DexScreener] {} yeni havuz keşfedildi",
                        "🌐".cyan(), count
                    );
                }
            }
            Err(e) => {
                eprintln!(
                    "  {} [DexScreener] API hatası — GeckoTerminal'e geçiliyor: {}",
                    "⚠️".yellow(), e
                );

                // Kaynak 2: GeckoTerminal (fallback)
                match discover_gecko_terminal(config).await {
                    Ok(pools) => {
                        let count = pools.len();
                        let mut reg = registry.write();
                        for pool in pools {
                            reg.enqueue_pending(pool);
                        }
                        if count > 0 {
                            reg.stats.api_discoveries += count as u64;
                            eprintln!(
                                "  {} [GeckoTerminal] {} yeni havuz keşfedildi (fallback)",
                                "🦎".green(), count
                            );
                        }
                    }
                    Err(e2) => {
                        eprintln!(
                            "  {} [GeckoTerminal] Fallback da başarısız: {}",
                            "❌".red(), e2
                        );
                    }
                }
            }
        }

        tokio::time::sleep(interval).await;
    }
}

/// DexScreener API sorgusu — düşük fee'li WETH çiftlerini keşfet
async fn discover_dexscreener(config: &DiscoveryConfig) -> Result<Vec<PendingPool>> {
    let url = format!(
        "https://api.dexscreener.com/latest/dex/tokens/{}",
        config.weth_address
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| eyre::eyre!("HTTP istemci hatası: {}", e))?;

    let resp = client
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| eyre::eyre!("DexScreener istek hatası: {}", e))?;

    if !resp.status().is_success() {
        return Err(eyre::eyre!("DexScreener HTTP {}", resp.status()));
    }

    let json: serde_json::Value = resp.json().await
        .map_err(|e| eyre::eyre!("DexScreener JSON hatası: {}", e))?;

    parse_dexscreener_pools(&json, config)
}

fn parse_dexscreener_pools(json: &serde_json::Value, config: &DiscoveryConfig) -> Result<Vec<PendingPool>> {
    let whitelist = token_whitelist();
    let mut results = Vec::new();

    let pairs = json.get("pairs")
        .and_then(|v| v.as_array())
        .ok_or_else(|| eyre::eyre!("DexScreener yanıtında 'pairs' bulunamadı"))?;

    for pair in pairs {
        let chain_id = pair.get("chainId").and_then(|v| v.as_str()).unwrap_or("");
        if chain_id != "base" {
            continue;
        }

        let dex_id = pair.get("dexId").and_then(|v| v.as_str()).unwrap_or("");
        let dex_lower = dex_id.to_lowercase();
        let is_v3 = dex_lower.contains("uniswap")
            || dex_lower.contains("pancakeswap")
            || dex_lower.contains("sushiswap")
            || dex_lower.contains("aerodrome");
        if !is_v3 {
            continue;
        }

        let pair_address = pair.get("pairAddress").and_then(|v| v.as_str()).unwrap_or("");
        let base_addr_str = pair.get("baseToken")
            .and_then(|v| v.get("address"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let quote_addr_str = pair.get("quoteToken")
            .and_then(|v| v.get("address"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Token adresleri parse
        let base_addr = match base_addr_str.parse::<Address>() {
            Ok(a) => a,
            Err(_) => continue,
        };
        let quote_addr = match quote_addr_str.parse::<Address>() {
            Ok(a) => a,
            Err(_) => continue,
        };
        let pool_addr = match pair_address.parse::<Address>() {
            Ok(a) => a,
            Err(_) => continue,
        };

        // Whitelist kontrolü
        if !whitelist.contains(&base_addr) || !whitelist.contains(&quote_addr) {
            continue;
        }

        // WETH çifti kontrolü
        let has_weth = base_addr == config.weth_address || quote_addr == config.weth_address;
        if !has_weth {
            continue;
        }

        // Likidite filtresi
        let liq = pair.get("liquidity")
            .and_then(|v| v.get("usd"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        if liq < config.min_liquidity_usd {
            continue;
        }

        // Hacim filtresi
        let vol24 = pair.get("volume")
            .and_then(|v| v.get("h24"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        if vol24 < config.min_volume_24h_usd {
            continue;
        }

        // Fee filtresi
        let fee_tier = pair.get("feeTier").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let fee_bps = (fee_tier * 100.0).round() as u32;
        if fee_bps > config.max_fee_bps {
            continue;
        }

        // DEX type
        let dex_type = match infer_dex_type_from_id(dex_id) {
            Some(dt) => dt,
            None => continue,
        };

        let token0_is_weth = base_addr == config.weth_address
            || (quote_addr != config.weth_address && base_addr < quote_addr);

        let (t0_decimals, t1_decimals, qtoken) = if token0_is_weth {
            (18u8, infer_decimals(&quote_addr), quote_addr)
        } else {
            (infer_decimals(&base_addr), 18u8, base_addr)
        };

        let tick_spacing = infer_tick_spacing_from_fee(dex_id, fee_bps);

        let btoken = if token0_is_weth { base_addr } else { quote_addr };
        let pool_config = PoolConfig {
            address: pool_addr,
            name: format!("{}-WETH", dex_id),
            fee_bps,
            fee_fraction: fee_bps as f64 / 10_000.0,
            token0_decimals: t0_decimals,
            token1_decimals: t1_decimals,
            dex: dex_type,
            token0_is_weth,
            tick_spacing,
            quote_token_address: qtoken,
            base_token_address: btoken,
        };

        results.push(PendingPool {
            config: pool_config,
            source: DiscoverySource::DexScreener,
            discovered_at: Instant::now(),
            score: vol24 * 0.001, // Ön skor: hacim bazlı
        });
    }

    Ok(results)
}

/// GeckoTerminal API sorgusu — DexScreener fallback
async fn discover_gecko_terminal(config: &DiscoveryConfig) -> Result<Vec<PendingPool>> {
    // GeckoTerminal v2 API: trending pools on Base
    let url = format!(
        "{}/networks/base/trending_pools",
        GECKO_TERMINAL_API
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| eyre::eyre!("HTTP istemci hatası: {}", e))?;

    let resp = client
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| eyre::eyre!("GeckoTerminal istek hatası: {}", e))?;

    if !resp.status().is_success() {
        return Err(eyre::eyre!("GeckoTerminal HTTP {}", resp.status()));
    }

    let json: serde_json::Value = resp.json().await
        .map_err(|e| eyre::eyre!("GeckoTerminal JSON hatası: {}", e))?;

    parse_gecko_terminal_pools(&json, config)
}

fn parse_gecko_terminal_pools(
    json: &serde_json::Value,
    config: &DiscoveryConfig,
) -> Result<Vec<PendingPool>> {
    let whitelist = token_whitelist();
    let mut results = Vec::new();

    let data = json.get("data")
        .and_then(|v| v.as_array())
        .ok_or_else(|| eyre::eyre!("GeckoTerminal yanıtında 'data' bulunamadı"))?;

    for pool_data in data {
        let attributes = match pool_data.get("attributes") {
            Some(a) => a,
            None => continue,
        };

        let pool_address_str = attributes.get("address")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let pool_addr = match pool_address_str.parse::<Address>() {
            Ok(a) => a,
            Err(_) => continue,
        };

        // Token bilgilerini al
        let base_addr_str = attributes.get("base_token_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .strip_prefix("base_")
            .unwrap_or("");
        let quote_addr_str = attributes.get("quote_token_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .strip_prefix("base_")
            .unwrap_or("");

        let base_addr = match base_addr_str.parse::<Address>() {
            Ok(a) => a,
            Err(_) => continue,
        };
        let quote_addr = match quote_addr_str.parse::<Address>() {
            Ok(a) => a,
            Err(_) => continue,
        };

        // Whitelist kontrolü
        if !whitelist.contains(&base_addr) || !whitelist.contains(&quote_addr) {
            continue;
        }

        // WETH çifti kontrolü
        let has_weth = base_addr == config.weth_address || quote_addr == config.weth_address;
        if !has_weth {
            continue;
        }

        // Hacim kontrolü
        let vol24 = attributes.get("volume_usd")
            .and_then(|v| v.get("h24"))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        if vol24 < config.min_volume_24h_usd {
            continue;
        }

        // Reserve/likidite kontrolü
        let reserve_usd = attributes.get("reserve_in_usd")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        if reserve_usd < config.min_liquidity_usd {
            continue;
        }

        // DEX name'den tip tahmin et
        let dex_name = pool_data.get("relationships")
            .and_then(|v| v.get("dex"))
            .and_then(|v| v.get("data"))
            .and_then(|v| v.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let dex_type = match infer_dex_type_from_id(dex_name) {
            Some(dt) => dt,
            None => continue,
        };

        let token0_is_weth = base_addr == config.weth_address;
        let (t0_dec, t1_dec, qtoken) = if token0_is_weth {
            (18u8, infer_decimals(&quote_addr), quote_addr)
        } else {
            (infer_decimals(&base_addr), 18u8, base_addr)
        };

        let btoken = if token0_is_weth { base_addr } else { quote_addr };
        let pool_config = PoolConfig {
            address: pool_addr,
            name: format!("Gecko-WETH"),
            fee_bps: 5, // GeckoTerminal genelde fee bilgisi vermez, 0.05% varsay
            fee_fraction: 0.0005,
            token0_decimals: t0_dec,
            token1_decimals: t1_dec,
            dex: dex_type,
            token0_is_weth,
            tick_spacing: 10,
            quote_token_address: qtoken,
            base_token_address: btoken,
        };

        results.push(PendingPool {
            config: pool_config,
            source: DiscoverySource::GeckoTerminal,
            discovered_at: Instant::now(),
            score: vol24 * 0.001,
        });
    }

    Ok(results)
}

// ─────────────────────────────────────────────────────────────────────────────
// [ADIM 3] HOT-RELOAD — Bekleyen Havuzları Canlı Sisteme Enjekte Et
// ─────────────────────────────────────────────────────────────────────────────

/// Bekleyen havuzları aktif listeye ekle ve gerekli pair_combo'ları oluştur.
/// Ana döngü tarafından her blokta çağrılır.
///
/// Dönüş: Eklenen yeni havuz sayısı
pub fn apply_pending_updates(
    registry: &Arc<RwLock<LivePoolRegistry>>,
    pools: &mut Vec<PoolConfig>,
    states: &mut Vec<SharedPoolState>,
    pair_combos: &mut Vec<PairCombo>,
) -> usize {
    let pending = {
        let mut reg = registry.write();
        reg.take_pending()
    };

    if pending.is_empty() {
        return 0;
    }

    let mut added = 0;

    for pending_pool in pending {
        let new_addr = pending_pool.config.address;

        // Mevcut havuzlarda zaten var mı?
        if pools.iter().any(|p| p.address == new_addr) {
            continue;
        }

        let new_idx = pools.len();
        let new_pool = pending_pool.config;

        // Aynı quote token'a sahip mevcut havuzlarla pair combo oluştur
        for (existing_idx, existing_pool) in pools.iter().enumerate() {
            if existing_pool.quote_token_address == new_pool.quote_token_address
                && existing_pool.address != new_pool.address
            {
                // Aynı token çifti, farklı DEX → arbitraj çifti
                let pair_name = format!("WETH/{:.8}", format!("{}", new_pool.quote_token_address));
                pair_combos.push(PairCombo {
                    pair_name,
                    pool_a_idx: existing_idx,
                    pool_b_idx: new_idx,
                });
            }
        }

        pools.push(new_pool);
        states.push(Arc::new(RwLock::new(PoolState::default())));

        {
            let mut reg = registry.write();
            reg.active_flags.push(true);
            reg.stats.pools_activated += 1;
        }

        added += 1;
    }

    if added > 0 {
        eprintln!(
            "  {} [Hot-Reload] {} yeni havuz aktif izlemeye eklendi (toplam: {})",
            "🔥".green(), added, pools.len(),
        );
    }

    added
}

// ─────────────────────────────────────────────────────────────────────────────
// [ADIM 4] GARBAGE COLLECTOR — Soğuk Havuzları Temizle
// ─────────────────────────────────────────────────────────────────────────────

/// Hareketsiz havuzları deaktive et ve yerine sıcak havuzlar ekle.
/// Ana döngü tarafından periyodik olarak çağrılır.
///
/// Dönüş: Deaktive edilen havuz sayısı
pub fn run_garbage_collector(
    registry: &Arc<RwLock<LivePoolRegistry>>,
    pools: &[PoolConfig],
    current_block: u64,
    config: &DiscoveryConfig,
) -> Vec<usize> {
    let mut reg = registry.write();

    // GC zamanı geldi mi?
    if current_block.saturating_sub(reg.last_gc_block) < config.gc_interval_blocks {
        return Vec::new();
    }

    let deactivated = reg.garbage_collect(
        pools,
        current_block,
        config.cooldown_blocks,
        config.max_active_pools,
    );

    if !deactivated.is_empty() {
        eprintln!(
            "  {} [GC] {} havuz uyku moduna alındı (blok #{}): {:?}",
            "🧹".yellow(),
            deactivated.len(),
            current_block,
            deactivated.iter()
                .filter_map(|&idx| pools.get(idx).map(|p| p.name.clone()))
                .collect::<Vec<_>>(),
        );
    }

    reg.last_gc_block = current_block;

    deactivated
}

// ─────────────────────────────────────────────────────────────────────────────
// [ADIM 5] OPPORTUNITY SCORER — Kâr Potansiyeli Puanlaması
// ─────────────────────────────────────────────────────────────────────────────

/// Tüm aktif havuzların fırsat skorlarını yeniden hesapla.
/// Formül: Score = (Son 1 Saatlik Hacim × Spread Dalgalanması) / Havuz Komisyonu
///
/// Ana döngü tarafından periyodik olarak (her ~10 dakikada bir) çağrılır.
pub fn update_scores(
    registry: &Arc<RwLock<LivePoolRegistry>>,
    pools: &[PoolConfig],
    current_block: u64,
    config: &DiscoveryConfig,
) -> bool {
    let should_update = {
        let reg = registry.read();
        current_block.saturating_sub(reg.last_score_block) >= config.score_interval_blocks
    };

    if !should_update {
        return false;
    }

    let mut reg = registry.write();
    reg.recalculate_scores(pools, current_block);

    // En iyi 5 havuzu logla
    let top5 = reg.top_scored_indices(5);
    if !top5.is_empty() {
        let scores_str: Vec<String> = top5.iter()
            .filter_map(|&idx| {
                let name = pools.get(idx).map(|p| p.name.clone())?;
                let score = reg.scores.get(&idx).map(|s| s.score)?;
                Some(format!("{}={:.0}", name, score))
            })
            .collect();
        eprintln!(
            "  {} [Scorer] Skor güncellendi — Top 5: {}",
            "📊".cyan(),
            scores_str.join(" | "),
        );
    }

    true
}

/// Havuz spread gözlemlerini kaydet (ana döngüden her blokta çağrılır)
pub fn record_spread_observation(
    registry: &Arc<RwLock<LivePoolRegistry>>,
    pool_a_idx: usize,
    pool_b_idx: usize,
    states: &[SharedPoolState],
) {
    let sa = states[pool_a_idx].read();
    let sb = states[pool_b_idx].read();

    if !sa.is_active() || !sb.is_active() {
        return;
    }

    let spread = (sa.eth_price_usd - sb.eth_price_usd).abs();
    let min_p = sa.eth_price_usd.min(sb.eth_price_usd);
    if min_p <= 0.0 {
        return;
    }

    let spread_pct = (spread / min_p) * 100.0;

    let mut reg = registry.write();
    reg.record_spread(pool_a_idx, spread_pct);
    reg.record_spread(pool_b_idx, spread_pct);
}

/// Swap aktivitesini kaydet (swap event dinleyiciden veya state_sync'ten)
#[allow(dead_code)]
pub fn record_swap_activity(
    registry: &Arc<RwLock<LivePoolRegistry>>,
    pool_idx: usize,
    block_number: u64,
) {
    let mut reg = registry.write();
    reg.record_swap(pool_idx, block_number);
}

// ─────────────────────────────────────────────────────────────────────────────
// Yardımcı Fonksiyonlar
// ─────────────────────────────────────────────────────────────────────────────

fn infer_dex_type_from_id(dex_id: &str) -> Option<DexType> {
    let lower = dex_id.to_lowercase();

    match lower.as_str() {
        "pancakeswap" | "pancakeswap-v3" | "pancakeswap_v3" => Some(DexType::PancakeSwapV3),
        "aerodrome" | "aerodrome-slipstream" | "aerodrome_slipstream" | "aerodrome-cl" => Some(DexType::Aerodrome),
        "uniswap" | "uniswap-v3" | "uniswap_v3" | "uniswapv3" => Some(DexType::UniswapV3),
        "sushiswap" | "sushiswap-v3" | "sushiswap_v3" => Some(DexType::UniswapV3),
        _ => {
            if lower.contains("pancake") {
                Some(DexType::PancakeSwapV3)
            } else if lower.contains("aerodrome") || lower.contains("slipstream") {
                Some(DexType::Aerodrome)
            } else if lower.contains("uniswap") || lower.contains("sushi") {
                Some(DexType::UniswapV3)
            } else {
                None
            }
        }
    }
}

fn infer_tick_spacing_from_fee(dex_id: &str, fee_bps: u32) -> i32 {
    let dex_lower = dex_id.to_lowercase();
    if dex_lower.contains("pancake") {
        match fee_bps { 1 => 1, 5 => 10, 25 => 50, 100 => 200, _ => 10 }
    } else {
        match fee_bps { 1 => 1, 5 => 10, 30 => 60, 100 => 200, _ => 10 }
    }
}

/// Keşif motoru istatistiklerini güzel formatlı çıktı olarak yazdır
pub fn print_discovery_stats(registry: &Arc<RwLock<LivePoolRegistry>>, pools: &[PoolConfig]) {
    let reg = registry.read();
    let active_count = reg.active_flags.iter().filter(|&&a| a).count();
    let sleeping_count = reg.sleeping_pools.len();

    println!("  {} ─── Keşif Motoru İstatistikleri ─────────", "│".yellow());
    println!("  {}  Toplam Keşfedilen   : {}", "│".yellow(), reg.stats.total_discovered);
    println!("  {}  Factory Eventleri   : {}", "│".yellow(), reg.stats.factory_events_received);
    println!("  {}  API Keşifleri       : {}", "│".yellow(), reg.stats.api_discoveries);
    println!("  {}  Aktif Havuz         : {}", "│".yellow(), active_count);
    println!("  {}  Uyuyan Havuz        : {}", "│".yellow(), sleeping_count);
    println!("  {}  GC Temizlenen       : {}", "│".yellow(), reg.stats.pools_garbage_collected);
    println!("  {}  Skor Güncelleme     : {}", "│".yellow(), reg.stats.score_recalculations);

    // Top 3 skorlu havuz
    let top3 = reg.top_scored_indices(3);
    if !top3.is_empty() {
        let top_str: Vec<String> = top3.iter()
            .filter_map(|&idx| {
                let name = pools.get(idx).map(|p| p.name.as_str())?;
                let score = reg.scores.get(&idx)?.score;
                Some(format!("{}: {:.0}", name, score))
            })
            .collect();
        println!("  {}  En İyi Havuzlar     : {}", "│".yellow(), top_str.join(", "));
    }
}
