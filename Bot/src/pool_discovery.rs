// ============================================================================
//  POOL DISCOVERY v26.0 — Kutsal Üçlü Keşif Motoru (Holy Trinity)
//
//  3 Aşamalı Konsensüs & Doğrulama Mimarisi:
//
//  Aşama 1: Veri Toplama
//    ✓ DexScreener API — Yüksek hacimli havuzları getirir
//    ✓ GeckoTerminal API — V3 etiketlerini (labels) çapraz doğrular
//
//  Aşama 2: Off-Chain Filtre
//    ✓ HashMap dedup + veri zenginleştirme (merge)
//    ✓ V3/CL label + isim analizi (whitelist/blacklist)
//    ✓ Hacim ($10K+), likidite ($50K+), fee (≤%0.05) filtreleri
//
//  Aşama 3: On-Chain RPC Doğrulama (Nihai Yargıç)
//    ✓ slot0() eth_call ile gerçek V3 kanıtı
//    ✓ Paralel sorgulama (futures::join_all)
//    ✓ 2s timeout per call
//    ✓ execution reverted → havuz reddedilir
//
//  Pipeline:
//    DexScreener ──┐
//                  ├─► merge ─► off_chain_filter ─► on_chain_validate
//    GeckoTerminal ┘                                        │
//                                                           ▼
//                                              match_arbitrage_pairs
//                                                           │
//                                                           ▼
//                                                  matched_pools.json
// ============================================================================

use eyre::Result;
use serde::{Deserialize, Serialize};
use colored::*;
use std::collections::HashMap;
use alloy::primitives::Address;
use futures_util::future::join_all;

use crate::types::{DexType, PoolConfig};

// ─── Sabitler ───
const BASE_WETH_LOWER: &str = "0x4200000000000000000000000000000000000006";
const MATCHED_POOLS_PATH: &str = "matched_pools.json";
const CORE_POOLS_PATH: &str = "core_pools.json";

/// slot0() fonksiyon seçicisi (4 byte): keccak256("slot0()")[0..4]
const SLOT0_SELECTOR: [u8; 4] = [0x38, 0x50, 0xc7, 0xbd];

/// On-Chain doğrulama için eth_call timeout (milisaniye)
/// v29.0: 2000ms → 500ms. Bayat veri beklemek yerine hızlı başarısızlık.
const ONCHAIN_CALL_TIMEOUT_MS: u64 = 500;

// ─────────────────────────────────────────────────────────────────────────────
// DexScreener API Yanıt Yapıları
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct DexScreenerResponse {
    pairs: Option<Vec<DexPair>>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DexPair {
    chain_id: String,
    dex_id: String,
    pair_address: String,
    /// DexScreener etiketleri — havuz tipini belirler (ör: ["v3"], ["v2"], ["CLAMM"])
    labels: Option<Vec<String>>,
    base_token: DexToken,
    quote_token: DexToken,
    liquidity: Option<DexLiquidity>,
    volume: Option<DexVolume>,
    fee_tier: Option<f64>,
}

#[derive(Debug, Deserialize, Clone)]
struct DexToken {
    address: String,
    symbol: String,
}

#[derive(Debug, Deserialize, Clone)]
struct DexLiquidity {
    usd: Option<f64>,
}

#[derive(Debug, Deserialize, Clone)]
struct DexVolume {
    h24: Option<f64>,
}

// ─────────────────────────────────────────────────────────────────────────────
// GeckoTerminal API Yanıt Yapıları
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct GeckoResponse {
    data: Option<Vec<GeckoPoolData>>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GeckoPoolData {
    /// Format: "base_0xADDRESS"
    id: Option<String>,
    attributes: Option<GeckoPoolAttributes>,
    relationships: Option<GeckoRelationships>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GeckoPoolAttributes {
    address: Option<String>,
    name: Option<String>,
    /// GeckoTerminal'in pool_created_at alanı (debug için)
    pool_created_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GeckoRelationships {
    dex: Option<GeckoDexRelation>,
}

#[derive(Debug, Deserialize)]
struct GeckoDexRelation {
    data: Option<GeckoDexData>,
}

#[derive(Debug, Deserialize)]
struct GeckoDexData {
    /// GeckoTerminal DEX ID — ör: "uniswap_v3_base", "aerodrome-slipstream"
    id: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Birleştirilmiş Havuz Verisi (Internal)
// ─────────────────────────────────────────────────────────────────────────────

/// Keşfedilen havuz bilgisi — tüm veri kaynaklarından birleştirilmiş
#[derive(Debug, Clone)]
pub struct DiscoveredPool {
    pub address: String,
    pub dex: String,
    /// Birleşik etiketler (DexScreener + GeckoTerminal)
    pub labels: Option<Vec<String>>,
    /// GeckoTerminal havuz adı (V3 ipuçları için)
    pub gecko_name: Option<String>,
    pub base_token_address: String,
    pub base_symbol: String,
    pub quote_token_address: String,
    pub quote_symbol: String,
    pub liquidity_usd: f64,
    pub volume_24h: f64,
    pub fee_tier: Option<f64>,
    /// Veri kaynakları: "dexscreener", "geckoterminal", "both"
    pub source: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// matched_pools.json Yapıları
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenInfo {
    pub address: String,
    pub symbol: String,
    pub decimals: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchedPoolEntry {
    pub address: String,
    pub dex_id: String,
    pub fee_bps: u32,
    pub tick_spacing: i32,
    pub liquidity_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchedPair {
    pub pair_name: String,
    pub base_token: TokenInfo,
    pub quote_token: TokenInfo,
    pub weth_is_token0: bool,
    pub pools: Vec<MatchedPoolEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchedPoolsConfig {
    pub version: String,
    pub chain_id: u64,
    pub updated_at: String,
    pub matched_pairs: Vec<MatchedPair>,
}

/// Ana döngü için çift eşleştirme indeksleri
pub struct PairCombo {
    pub pair_name: String,
    pub pool_a_idx: usize,
    pub pool_b_idx: usize,
}

// ─────────────────────────────────────────────────────────────────────────────
// Yardımcı Fonksiyonlar
// ─────────────────────────────────────────────────────────────────────────────

fn normalize_pair_key(addr_a: &str, addr_b: &str) -> (String, String) {
    let a = addr_a.to_lowercase();
    let b = addr_b.to_lowercase();
    if a < b { (a, b) } else { (b, a) }
}

fn fee_tier_to_bps(fee_tier: Option<f64>) -> u32 {
    fee_tier.map(|f| (f * 100.0).round() as u32).unwrap_or(0)
}

/// V3/Konsantre Likidite (CL) doğrulaması — Off-Chain Aşama.
///
/// Üç katmanlı karar ağacı:
///   1. Labels beyaz liste: v3, v4, cl, clamm, clpool, slipstream → V3 kesin
///   2. Labels kara liste: v1, v2, stable, vamm → V2 kesin, REDDET
///   3. Fee-tier fallback: Aerodrome/PancakeSwap labels döndürmez,
///      fee_tier varlığı V3 (CL) göstergesidir.
///   4. İsim analizi: GeckoTerminal havuz adında "v3", "cl", "slipstream" geçer mi?
///
/// Hiçbir koşul sağlanmazsa V2 varsayılır ve havuz reddedilir.
fn is_v3_pool(labels: &Option<Vec<String>>, dex_id: &str, fee_tier: &Option<f64>, gecko_name: &Option<String>) -> bool {
    const V3_WHITELIST: &[&str] = &["v3", "v4", "cl", "clamm", "clpool", "slipstream"];
    const V2_BLACKLIST: &[&str] = &["v1", "v2", "stable", "vamm"];

    // 1. Label tabanlı karar
    if let Some(tags) = labels {
        for tag in tags {
            let lower = tag.to_lowercase();
            if V3_WHITELIST.iter().any(|w| lower == *w) { return true; }
            if V2_BLACKLIST.iter().any(|b| lower == *b) { return false; }
        }
    }

    // 2. GeckoTerminal havuz ismi analizi
    if let Some(name) = gecko_name {
        let name_lower = name.to_lowercase();
        if V3_WHITELIST.iter().any(|w| name_lower.contains(w)) {
            return true;
        }
        if V2_BLACKLIST.iter().any(|b| name_lower.contains(b)) {
            return false;
        }
    }

    // 3. Aerodrome: labels yok, fee_tier varsa Slipstream (CL)
    let dex_lower = dex_id.to_lowercase();
    if dex_lower.contains("aerodrome") || dex_lower.contains("slipstream") {
        return fee_tier.is_some();
    }

    // 4. PancakeSwap/SushiSwap: labels yok, fee_tier varsa V3
    if dex_lower.contains("pancake") || dex_lower.contains("sushi") {
        return fee_tier.is_some();
    }

    false // Bilinmeyen → V2 varsay → REDDET
}

fn infer_tick_spacing(dex_id: &str, fee_bps: u32) -> i32 {
    let dex_lower = dex_id.to_lowercase();
    if dex_lower.contains("pancakeswap") || dex_lower.contains("pancake") {
        match fee_bps { 1 => 1, 5 => 10, 25 => 50, 100 => 200, _ => 10 }
    } else {
        match fee_bps { 1 => 1, 5 => 10, 30 => 60, 100 => 200, _ => 10 }
    }
}

#[allow(clippy::if_same_then_else)]
fn infer_token_decimals(address: &str) -> u8 {
    let lower = address.to_lowercase();
    if lower.ends_with("0000000000000000000006") { 18 }
    else if lower.contains("cbb7c0000ab88b473b1f5afd9ef808440eed33bf") { 8 }
    else if lower.contains("833589fcd6edb6e08f4c7c32d4f71b54bda02913") { 6 }
    else if lower.contains("d9aaec86b65d86f6a7b5b1b0c42ffa531710b6ca") { 6 }
    else if lower.contains("50c5725949a6f0c72e6c4a641f24049a917db0cb") { 18 }
    else if lower.contains("2ae3f1ec7f1f5012cfeab0185bfc7aa3cf0dec22") { 18 }
    else { 18 }
}

/// v23.0 (Y-3): Bilinmeyen DEX ID'leri artık `None` döner (eski: UniswapV3 varsayımı).
/// Bilinmeyen DEX'ler farklı slot0 struct yapısına sahip olabilir;
/// yanlış ABI ile state parse etmek hatalı fiyat ve zarar riski doğurur.
fn infer_dex_type(dex_id: &str) -> Option<DexType> {
    let lower = dex_id.to_lowercase();
    
    // Tam eşleşme öncelikli — DexScreener'ın bilinen DEX ID formatları
    match lower.as_str() {
        "pancakeswap" | "pancakeswap-v3" | "pancakeswap_v3" => Some(DexType::PancakeSwapV3),
        "aerodrome" | "aerodrome-slipstream" | "aerodrome_slipstream" | "aerodrome-cl" => Some(DexType::Aerodrome),
        "uniswap" | "uniswap-v3" | "uniswap_v3" | "uniswapv3" => Some(DexType::UniswapV3),
        "sushiswap" | "sushiswap-v3" | "sushiswap_v3" => Some(DexType::UniswapV3),
        _ => {
            // Fallback: substring eşleşme (yeni DEX ID'ler için)
            if lower.contains("pancake") {
                Some(DexType::PancakeSwapV3)
            } else if lower.contains("aerodrome") || lower.contains("slipstream") {
                Some(DexType::Aerodrome)
            } else {
                // v23.0 (Y-3): Bilinmeyen DEX — havuz atlanır (eski: sessiz UniswapV3 fallback)
                eprintln!(
                    "  ⚠️  Unknown DEX ID '{}' — pool skipped (safe mode)",
                    dex_id
                );
                None
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AŞAMA 1a: DexScreener Veri Toplama
// ─────────────────────────────────────────────────────────────────────────────

async fn fetch_dexscreener_pools() -> Result<Vec<DexPair>> {
    let url = format!(
        "https://api.dexscreener.com/latest/dex/tokens/{}",
        BASE_WETH_LOWER
    );

    eprintln!("  {} [Phase 1a] Querying DexScreener API...", "🔍".cyan());

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| eyre::eyre!("HTTP client error: {}", e))?;

    let resp = client
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| eyre::eyre!("DexScreener API error: {}", e))?;

    // Rate limiting — 429 Too Many Requests durumunda backoff
    if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let retry_after = resp.headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(60);
        eprintln!(
            "  ⚠️ [DexScreener] Rate limit (429) — waiting {}s",
            retry_after
        );
        tokio::time::sleep(std::time::Duration::from_secs(retry_after)).await;
        return Err(eyre::eyre!("Rate limit (429)"));
    }

    let resp: DexScreenerResponse = resp.json()
        .await
        .map_err(|e| eyre::eyre!("DexScreener JSON parse error: {}", e))?;

    let pairs: Vec<DexPair> = resp.pairs.unwrap_or_default()
        .into_iter()
        .filter(|p| p.chain_id == "base")
        // DEX Beyaz Listesi (V3 ABI uyumlu)
        .filter(|p| {
            let dex = p.dex_id.to_lowercase();
            dex.contains("uniswap")
                || dex.contains("pancakeswap")
                || dex.contains("sushiswap")
                || dex.contains("aerodrome")
        })
        .collect();

    eprintln!(
        "  {} [DexScreener] {} pools fetched (Base, whitelisted DEXes)",
        "✅".green(), pairs.len()
    );

    Ok(pairs)
}

// ─────────────────────────────────────────────────────────────────────────────
// AŞAMA 1b: GeckoTerminal Veri Toplama
// ─────────────────────────────────────────────────────────────────────────────

async fn fetch_geckoterminal_pools() -> Vec<GeckoPoolData> {
    // Birden fazla sayfa çekerek daha fazla veri topla
    let pages = [1, 2, 3];
    let mut all_pools: Vec<GeckoPoolData> = Vec::new();

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  ⚠️ [GeckoTerminal] HTTP client creation failed: {} — skipping", e);
            return Vec::new();
        }
    };

    for page in pages {
        let url = format!(
            "https://api.geckoterminal.com/api/v2/networks/base/pools?page={}&sort=h24_tx_count_desc",
            page
        );

        let resp = match client
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("  ⚠️ [GeckoTerminal] Page {} API error: {} — continuing", page, e);
                continue;
            }
        };

        // Rate limit handling
        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            eprintln!("  ⚠️ [GeckoTerminal] Rate limit (429) — skipping page {}", page);
            // Kısa bekleme ve devam
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            continue;
        }

        if !resp.status().is_success() {
            eprintln!(
                "  ⚠️ [GeckoTerminal] Page {} HTTP {} — skipping",
                page, resp.status()
            );
            continue;
        }

        match resp.json::<GeckoResponse>().await {
            Ok(gecko_resp) => {
                let pools = gecko_resp.data.unwrap_or_default();
                all_pools.extend(pools);
            }
            Err(e) => {
                eprintln!("  ⚠️ [GeckoTerminal] Page {} JSON parse error: {} — skipping", page, e);
                continue;
            }
        }

        // Sayfalar arası kısa bekleme (rate limit koruması)
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }

    eprintln!(
        "  {} [GeckoTerminal] {} pools fetched (Base, {} pages)",
        if all_pools.is_empty() { "⚠️" } else { "✅" },
        all_pools.len(),
        pages.len()
    );

    all_pools
}

/// GeckoTerminal DEX ID'sinden V3 etiketi çıkar
/// Ör: "uniswap_v3_base" → Some("v3"), "aerodrome-slipstream" → Some("slipstream")
fn extract_gecko_v3_label(dex_id: &str) -> Option<String> {
    let lower = dex_id.to_lowercase();
    if lower.contains("v3") { return Some("v3".to_string()); }
    if lower.contains("slipstream") { return Some("slipstream".to_string()); }
    if lower.contains("clpool") || lower.contains("cl_pool") { return Some("clpool".to_string()); }
    if lower.contains("clamm") { return Some("clamm".to_string()); }
    // V2 tespiti
    if lower.contains("v2") { return Some("v2".to_string()); }
    None
}

// ─────────────────────────────────────────────────────────────────────────────
// AŞAMA 2: Veri Birleştirme (Data Merging & Deduplication)
// ─────────────────────────────────────────────────────────────────────────────

fn merge_pool_sources(dex_pairs: Vec<DexPair>, gecko_pools: Vec<GeckoPoolData>) -> Vec<DiscoveredPool> {
    let mut pool_map: HashMap<String, DiscoveredPool> = HashMap::new();

    // ── DexScreener verilerini ekle (primary kaynak) ──
    for p in dex_pairs {
        let addr_lower = p.pair_address.to_lowercase();
        pool_map.insert(addr_lower.clone(), DiscoveredPool {
            address: p.pair_address,
            dex: p.dex_id,
            labels: p.labels,
            gecko_name: None,
            base_token_address: p.base_token.address,
            base_symbol: p.base_token.symbol,
            quote_token_address: p.quote_token.address,
            quote_symbol: p.quote_token.symbol,
            liquidity_usd: p.liquidity.as_ref().and_then(|l| l.usd).unwrap_or(0.0),
            volume_24h: p.volume.as_ref().and_then(|v| v.h24).unwrap_or(0.0),
            fee_tier: p.fee_tier,
            source: "dexscreener".to_string(),
        });
    }

    // ── GeckoTerminal verilerini birleştir (zenginleştirme) ──
    let mut gecko_enriched = 0usize;
    for gecko in gecko_pools {
        let addr = match &gecko.attributes {
            Some(attrs) => match &attrs.address {
                Some(a) => a.to_lowercase(),
                None => continue,
            },
            None => continue,
        };

        // GeckoTerminal DEX ID'sinden V3 etiketi çıkar
        let gecko_label = gecko.relationships
            .as_ref()
            .and_then(|r| r.dex.as_ref())
            .and_then(|d| d.data.as_ref())
            .and_then(|d| d.id.as_ref())
            .and_then(|id| extract_gecko_v3_label(id));

        let gecko_name = gecko.attributes
            .as_ref()
            .and_then(|a| a.name.clone());

        if let Some(existing) = pool_map.get_mut(&addr) {
            // Mevcut DexScreener kaydını zenginleştir
            existing.gecko_name = gecko_name;
            existing.source = "both".to_string();

            // GeckoTerminal'den gelen etiketi ekle
            if let Some(label) = gecko_label {
                let labels = existing.labels.get_or_insert_with(Vec::new);
                if !labels.iter().any(|l| l.to_lowercase() == label.to_lowercase()) {
                    labels.push(label);
                }
            }
            gecko_enriched += 1;
        }
        // Not: GeckoTerminal'de olup DexScreener'da olmayan havuzlar eklenmez
        // çünkü DexScreener token bilgisi (base/quote) olmadan işe yaramaz.
    }

    eprintln!(
        "  {} [Merge] {} pools merged, {} enriched with GeckoTerminal",
        "🔗".cyan(), pool_map.len(), gecko_enriched
    );

    pool_map.into_values().collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// AŞAMA 3: Off-Chain Filtre (Zırh)
// ─────────────────────────────────────────────────────────────────────────────

fn off_chain_filter(pools: Vec<DiscoveredPool>) -> Vec<DiscoveredPool> {
    let before = pools.len();

    let mut filtered: Vec<DiscoveredPool> = pools
        .into_iter()
        // V3 Zırhı — V2 Zombi Bariyeri
        .filter(|p| {
            let pass = is_v3_pool(&p.labels, &p.dex, &p.fee_tier, &p.gecko_name);
            if !pass {
                eprintln!(
                    "  🛡️ V2 pool rejected: {} ({}) [labels: {:?}]", p.address, p.dex, p.labels
                );
            }
            pass
        })
        // Minimum likidite: $50K
        .filter(|p| p.liquidity_usd >= 50_000.0)
        // Minimum 24h hacim: $10K
        .filter(|p| p.volume_24h >= 10_000.0)
        // Maksimum fee: %0.05
        .filter(|p| p.fee_tier.is_none_or(|fee| fee <= 0.05))
        .collect();

    // Fee'ye göre artan sırala, eşit fee'de en yüksek hacmi tercih et
    filtered.sort_by(|a, b| {
        let fee_a = a.fee_tier.unwrap_or(f64::MAX);
        let fee_b = b.fee_tier.unwrap_or(f64::MAX);
        fee_a.partial_cmp(&fee_b)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                b.volume_24h
                    .partial_cmp(&a.volume_24h)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });

    // Makul üst sınır
    filtered.truncate(100);

    eprintln!(
        "  {} [Off-Chain Filter] {} → {} candidate pools (V3 + liquidity + volume + fee)",
        "🛡️".yellow(), before, filtered.len()
    );

    filtered
}

// ─────────────────────────────────────────────────────────────────────────────
// AŞAMA 4: On-Chain RPC Doğrulama (Nihai Yargıç)
// ─────────────────────────────────────────────────────────────────────────────

/// slot0() eth_call ile havuzun gerçekten V3/CL olduğunu doğrula.
///
/// Her aday havuza `slot0()` selector'ünü gönderir:
/// - Geçerli yanıt (≥ 64 byte) → %100 onaylanmış V3
/// - execution reverted / 0x / timeout → sahte V3, REDDET
///
/// Tüm sorgular `join_all` ile paralel çalışır.
pub async fn on_chain_validate(candidates: Vec<DiscoveredPool>) -> Vec<DiscoveredPool> {
    if candidates.is_empty() {
        return candidates;
    }

    // RPC_HTTP_URL env var'ından provider oluştur
    let rpc_url = match std::env::var("RPC_HTTP_URL") {
        Ok(url) if !url.is_empty() && !url.starts_with("https://your-") => url,
        _ => {
            eprintln!(
                "  ⚠️ [On-Chain] RPC_HTTP_URL not defined — On-Chain validation skipped (all candidates pass)"
            );
            return candidates;
        }
    };

    eprintln!(
        "  {} [Phase 3] On-Chain slot0() validation starting ({} candidates)...",
        "⛓️".cyan(), candidates.len()
    );

    // slot0() calldata: sadece 4-byte selector
    let calldata: alloy::primitives::Bytes = alloy::primitives::Bytes::from(SLOT0_SELECTOR.to_vec());

    // Her aday için asenkron doğrulama future'ı oluştur
    let futures: Vec<_> = candidates.iter().enumerate().map(|(i, pool)| {
        let rpc_url = rpc_url.clone();
        let pool_address_str = pool.address.clone();
        let pool_dex = pool.dex.clone();
        let calldata = calldata.clone();

        async move {
            // Her future kendi HTTP client'ını oluşturur (bağımsız timeout)
            let client = match reqwest::Client::builder()
                .timeout(std::time::Duration::from_millis(ONCHAIN_CALL_TIMEOUT_MS))
                .build()
            {
                Ok(c) => c,
                Err(_) => return (i, false),
            };

            // Pool adresini parse et
            let pool_address = match pool_address_str.parse::<Address>() {
                Ok(addr) => addr,
                Err(_) => return (i, false),
            };

            // eth_call JSON-RPC isteği oluştur
            let call_obj = serde_json::json!({
                "to": format!("{:?}", pool_address),
                "data": format!("0x{}", hex::encode(&calldata))
            });

            let rpc_request = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "eth_call",
                "params": [call_obj, "latest"],
                "id": i + 1
            });

            match client
                .post(&rpc_url)
                .header("Content-Type", "application/json")
                .json(&rpc_request)
                .send()
                .await
            {
                Ok(resp) => {
                    match resp.json::<serde_json::Value>().await {
                        Ok(json) => {
                            // Hata varsa → V2 veya sahte havuz
                            if json.get("error").is_some() {
                                eprintln!(
                                    "  ❌ [On-Chain] REJECTED: {} ({}) — execution reverted", pool_address_str, pool_dex
                                );
                                return (i, false);
                            }

                            // result alanını kontrol et
                            if let Some(result) = json.get("result").and_then(|r| r.as_str()) {
                                // "0x" veya çok kısa yanıt → geçersiz
                                let hex_data = result.strip_prefix("0x").unwrap_or(result);
                                if hex_data.is_empty() || hex_data.len() < 128 {
                                    // 128 hex karakter = 64 byte minimum (sqrtPriceX96 + tick)
                                    eprintln!(
                                        "  ❌ [On-Chain] REJECTED: {} ({}) — slot0 response too short ({}B)", pool_address_str, pool_dex, hex_data.len() / 2
                                    );
                                    return (i, false);
                                }

                                // Geçerli V3 havuzu!
                                (i, true)
                            } else {
                                (i, false)
                            }
                        }
                        Err(_) => (i, false),
                    }
                }
                Err(e) => {
                    // Timeout veya ağ hatası — güvenli tarafta kal, reddet
                    eprintln!(
                        "  ⏱️ [On-Chain] pool TIMEOUT/ERROR: {} ({}) — {}", pool_address_str, pool_dex, e
                    );
                    (i, false)
                }
            }
        }
    }).collect();

    // Tüm doğrulamaları paralel çalıştır
    let results = join_all(futures).await;

    // Geçen havuzları filtrele
    let mut passed_indices: Vec<usize> = results
        .into_iter()
        .filter_map(|(i, passed)| if passed { Some(i) } else { None })
        .collect();
    passed_indices.sort();

    let passed_count = passed_indices.len();
    let rejected_count = candidates.len() - passed_count;

    let validated: Vec<DiscoveredPool> = passed_indices
        .into_iter()
        .map(|i| candidates[i].clone())
        .collect();

    eprintln!(
        "  {} [On-Chain] {} pools VALIDATED ✅ | {} pools REJECTED ❌",
        "⛓️".green(), passed_count, rejected_count
    );

    validated
}

// ─────────────────────────────────────────────────────────────────────────────
// AŞAMA 5: Otonom Çift Eşleştirme
// ─────────────────────────────────────────────────────────────────────────────

/// Keşfedilen havuzları token çiftlerine göre grupla ve arbitraj eşleştirmesi yap
fn match_arbitrage_pairs(pools: &[DiscoveredPool]) -> Vec<MatchedPair> {
    let mut groups: HashMap<(String, String), Vec<&DiscoveredPool>> = HashMap::new();
    for pool in pools {
        let key = normalize_pair_key(&pool.base_token_address, &pool.quote_token_address);
        groups.entry(key).or_default().push(pool);
    }

    let mut matched = Vec::new();

    for ((token0_addr, token1_addr), group) in &groups {
        // Her DEX+Fee çifti ayrı bir slot — Fee Tier Isolation
        let mut dex_best: HashMap<String, &DiscoveredPool> = HashMap::new();
        for pool in group {
            let fee_bps = fee_tier_to_bps(pool.fee_tier);
            let dex_key = format!("{}_{}", pool.dex.to_lowercase(), fee_bps);
            match dex_best.get(&dex_key) {
                Some(existing) if existing.liquidity_usd >= pool.liquidity_usd => {}
                _ => { dex_best.insert(dex_key, pool); }
            }
        }

        // En az 2 farklı DEX gerekli
        if dex_best.len() < 2 {
            continue;
        }

        let mut selected: Vec<&DiscoveredPool> = dex_best.into_values().collect();
        // En düşük fee'li havuzu öne al
        selected.sort_by(|a, b| {
            let fee_a = a.fee_tier.unwrap_or(f64::MAX);
            let fee_b = b.fee_tier.unwrap_or(f64::MAX);
            fee_a.partial_cmp(&fee_b)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    b.liquidity_usd.partial_cmp(&a.liquidity_usd)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });

        // WETH ve quote token belirleme
        let is_t0_weth = token0_addr.eq_ignore_ascii_case(BASE_WETH_LOWER);
        let (weth_addr, quote_addr) = if is_t0_weth {
            (token0_addr.clone(), token1_addr.clone())
        } else {
            (token1_addr.clone(), token0_addr.clone())
        };

        // Sembolleri ilk havuzdan belirle
        let ref_pool = selected[0];
        let (weth_sym, quote_sym) = if ref_pool.base_token_address.to_lowercase() == BASE_WETH_LOWER {
            (ref_pool.base_symbol.clone(), ref_pool.quote_symbol.clone())
        } else {
            (ref_pool.quote_symbol.clone(), ref_pool.base_symbol.clone())
        };

        let pair_name = format!("{}/{}", weth_sym, quote_sym);
        matched.push(MatchedPair {
            pair_name,
            base_token: TokenInfo {
                address: weth_addr,
                symbol: weth_sym,
                decimals: 18,
            },
            quote_token: TokenInfo {
                address: quote_addr.clone(),
                symbol: quote_sym,
                decimals: infer_token_decimals(&quote_addr),
            },
            weth_is_token0: is_t0_weth,
            pools: selected.iter().map(|p| {
                let fee_bps = fee_tier_to_bps(p.fee_tier);
                MatchedPoolEntry {
                    address: p.address.clone(),
                    dex_id: p.dex.clone(),
                    fee_bps,
                    tick_spacing: infer_tick_spacing(&p.dex, fee_bps),
                    liquidity_usd: p.liquidity_usd,
                }
            }).collect(),
        });
    }

    // Toplam likiditeye göre sırala (azalan)
    matched.sort_by(|a, b| {
        let liq_a: f64 = a.pools.iter().map(|p| p.liquidity_usd).sum();
        let liq_b: f64 = b.pools.iter().map(|p| p.liquidity_usd).sum();
        liq_b.partial_cmp(&liq_a).unwrap_or(std::cmp::Ordering::Equal)
    });

    matched
}

// ─────────────────────────────────────────────────────────────────────────────
// CLI Keşif + JSON Çıktı (Ana Giriş Noktası)
// ─────────────────────────────────────────────────────────────────────────────

/// Havuzları keşfet, doğrula, eşleştir ve matched_pools.json olarak yaz
pub async fn cli_discover_pools() -> Result<()> {
    println!();
    println!("{}", "  ╔═══════════════════════════════════════════════════════════════════╗".cyan().bold());
    println!("{}", "  ║   Holy Trinity Discovery Engine v26.0 — Base Network               ║".cyan().bold());
    println!("{}", "  ║   DexScreener + GeckoTerminal + On-Chain RPC Validation           ║".cyan().bold());
    println!("{}", "  ╠═══════════════════════════════════════════════════════════════════╣".cyan().bold());
    println!();

    // ── AŞAMA 1: Paralel Veri Toplama ──
    // DexScreener ve GeckoTerminal'i aynı anda sorgula
    let (dex_result, gecko_pools) = tokio::join!(
        fetch_dexscreener_pools(),
        fetch_geckoterminal_pools()
    );

    // DexScreener başarısız olursa cache'e düş
    let dex_pairs = match dex_result {
        Ok(p) => p,
        Err(e) => {
            eprintln!("  ⚠️  DexScreener API error: {} — checking existing cache...", e);
            if std::path::Path::new(MATCHED_POOLS_PATH).exists() {
                eprintln!("  📦 Using existing matched_pools.json cache (DexScreener unreachable)");
                return Ok(());
            }
            return Err(eyre::eyre!("DexScreener unreachable and no existing cache found: {}", e));
        }
    };

    if dex_pairs.is_empty() {
        eprintln!("  {} No pools found.", "⚠️".yellow());
        return Ok(());
    }

    // ── AŞAMA 2: Veri Birleştirme ──
    let merged = merge_pool_sources(dex_pairs, gecko_pools);

    // ── AŞAMA 3: Off-Chain Filtre ──
    let candidates = off_chain_filter(merged);

    if candidates.is_empty() {
        eprintln!("  {} No candidates passed off-chain filter.", "⚠️".yellow());
        return Ok(());
    }

    // ── AŞAMA 4: On-Chain RPC Doğrulama (Nihai Yargıç) ──
    let validated = on_chain_validate(candidates).await;

    if validated.is_empty() {
        eprintln!("  {} No pools passed on-chain validation.", "⚠️".yellow());
        return Ok(());
    }

    eprintln!(
        "  {} {} pools validated, matching in progress...",
        "✅".green(), validated.len()
    );

    // ── AŞAMA 5: Çift Eşleştirme ──
    let matched_pairs = match_arbitrage_pairs(&validated);

    if matched_pairs.is_empty() {
        eprintln!("  {} No arbitrage pairs found (same pair needed on at least 2 DEXes).", "⚠️".yellow());
        return Ok(());
    }

    // Terminal çıktısı
    println!("{}", "  ╠═══════════════════════════════════════════════════════════════════╣".cyan().bold());
    println!("{}", "  ║   Validated Arbitrage Pairs (On-Chain Verified)                    ║".cyan().bold());
    println!("{}", "  ╠═══════════════════════════════════════════════════════════════════╣".cyan().bold());

    for (i, pair) in matched_pairs.iter().enumerate() {
        println!(
            "  {}  #{} {} ({} pools)",
            "║".cyan(), i + 1,
            pair.pair_name.white().bold(),
            pair.pools.len(),
        );
        for pool in &pair.pools {
            let fee_str = format!("{:.2}%", pool.fee_bps as f64 / 100.0);
            println!(
                "  {}    → {} | Fee: {} | Liq: ${:.0}K | {}",
                "║".cyan(),
                pool.dex_id.white(),
                fee_str,
                pool.liquidity_usd / 1000.0,
                pool.address.dimmed(),
            );
        }
    }

    println!("{}", "  ╚═══════════════════════════════════════════════════════════════════╝".cyan().bold());

    let config = MatchedPoolsConfig {
        version: "26.0".into(),
        chain_id: 8453,
        updated_at: chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string(),
        matched_pairs,
    };

    write_matched_pools_json(&config)?;

    println!(
        "\n  {} matched_pools.json written ({} pairs, {} total pools)",
        "✅".green(),
        config.matched_pairs.len(),
        config.matched_pairs.iter().map(|p| p.pools.len()).sum::<usize>(),
    );

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// JSON I/O
// ─────────────────────────────────────────────────────────────────────────────

fn write_matched_pools_json(config: &MatchedPoolsConfig) -> Result<()> {
    let json = serde_json::to_string_pretty(config)
        .map_err(|e| eyre::eyre!("JSON serialization error: {}", e))?;
    std::fs::write(MATCHED_POOLS_PATH, json)
        .map_err(|e| eyre::eyre!("matched_pools.json write error: {}", e))?;
    Ok(())
}

/// matched_pools.json dosyasını yükle
pub fn load_matched_pools() -> Result<MatchedPoolsConfig> {
    let content = std::fs::read_to_string(MATCHED_POOLS_PATH)
        .map_err(|e| eyre::eyre!("matched_pools.json read error: {} — Run `--discover-pools` first", e))?;
    let config: MatchedPoolsConfig = serde_json::from_str(&content)
        .map_err(|e| eyre::eyre!("matched_pools.json parse error: {}", e))?;
    Ok(config)
}

/// v29.0: Statik çekirdek havuz beyaz listesini yükle.
/// core_pools.json varsa matched_pools.json yerine bu kullanılır.
/// DexScreener bağımlılığını ortadan kaldırır.
pub fn load_core_pools() -> Option<MatchedPoolsConfig> {
    if !std::path::Path::new(CORE_POOLS_PATH).exists() {
        return None;
    }
    match std::fs::read_to_string(CORE_POOLS_PATH) {
        Ok(content) => {
            match serde_json::from_str::<MatchedPoolsConfig>(&content) {
                Ok(config) => {
                    eprintln!(
                        "  📦 core_pools.json loaded ({} pairs, {} pools) — skipping DexScreener", config.matched_pairs.len(),
                        config.matched_pairs.iter().map(|p| p.pools.len()).sum::<usize>(),
                    );
                    Some(config)
                }
                Err(e) => {
                    eprintln!("  ⚠️  core_pools.json parse error: {} — falling back to matched_pools.json", e);
                    None
                }
            }
        }
        Err(e) => {
            eprintln!("  ⚠️  core_pools.json read error: {} — falling back to matched_pools.json", e);
            None
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Runtime Dönüşümü — JSON → PoolConfig + PairCombo
// ─────────────────────────────────────────────────────────────────────────────

/// matched_pools.json'dan runtime yapıları oluştur:
///   - `Vec<PoolConfig>`: Tüm unique havuzların düz listesi (state_sync için)
///   - `Vec<PairCombo>`: Her arbitraj çifti için havuz indeks çiftleri
pub fn build_runtime(config: &MatchedPoolsConfig) -> Result<(Vec<PoolConfig>, Vec<PairCombo>)> {
    let mut all_pools: Vec<PoolConfig> = Vec::new();
    let mut pair_combos: Vec<PairCombo> = Vec::new();
    let mut address_to_idx: HashMap<String, usize> = HashMap::new();

    for pair in &config.matched_pairs {
        let quote_token_address = pair.quote_token.address.parse::<Address>()
            .map_err(|e| eyre::eyre!("Invalid quote token address '{}': {}", pair.quote_token.address, e))?;
        let base_token_address = pair.base_token.address.parse::<Address>()
            .map_err(|e| eyre::eyre!("Invalid base token address '{}': {}", pair.base_token.address, e))?;

        let mut pair_indices: Vec<usize> = Vec::new();

        for pool_entry in &pair.pools {
            let addr_lower = pool_entry.address.to_lowercase();

            let idx = if let Some(&existing_idx) = address_to_idx.get(&addr_lower) {
                existing_idx
            } else {
                let address = pool_entry.address.parse::<Address>()
                    .map_err(|e| eyre::eyre!("Invalid pool address '{}': {}", pool_entry.address, e))?;

                // v23.0 (Y-3): Bilinmeyen DEX'ler atlanır
                let dex_type = match infer_dex_type(&pool_entry.dex_id) {
                    Some(dt) => dt,
                    None => continue, // Bilinmeyen DEX — bu havuzu atla
                };

                let pool_config = PoolConfig {
                    address,
                    name: format!("{}-{}", pool_entry.dex_id, pair.pair_name),
                    fee_bps: pool_entry.fee_bps,
                    fee_fraction: pool_entry.fee_bps as f64 / 10_000.0,
                    token0_decimals: if pair.weth_is_token0 { pair.base_token.decimals } else { pair.quote_token.decimals },
                    token1_decimals: if pair.weth_is_token0 { pair.quote_token.decimals } else { pair.base_token.decimals },
                    dex: dex_type,
                    token0_is_weth: pair.weth_is_token0,
                    tick_spacing: pool_entry.tick_spacing,
                    quote_token_address,
                    base_token_address,
                };

                let idx = all_pools.len();
                all_pools.push(pool_config);
                address_to_idx.insert(addr_lower, idx);
                idx
            };

            pair_indices.push(idx);
        }

        // Her çift içindeki tüm 2-havuz kombinasyonlarını üret
        for i in 0..pair_indices.len() {
            for j in (i + 1)..pair_indices.len() {
                pair_combos.push(PairCombo {
                    pair_name: pair.pair_name.clone(),
                    pool_a_idx: pair_indices[i],
                    pool_b_idx: pair_indices[j],
                });
            }
        }
    }

    if all_pools.is_empty() {
        return Err(eyre::eyre!("no valid pools found in matched_pools.json"));
    }

    Ok((all_pools, pair_combos))
}

/// v10.0: Havuz listesi değiştikten sonra PairCombo'ları yeniden oluştur.
///
/// Pool validation/GC sonrası havuz indeksleri değiştiğinde çağrılır.
/// Aynı quote_token_address + base_token_address'e sahip havuzları gruplar
/// ve tüm 2-havuz kombinasyonlarını üretir.
pub fn rebuild_pair_combos(pools: &[PoolConfig]) -> Vec<PairCombo> {
    let mut pair_groups: HashMap<(Address, Address), Vec<usize>> = HashMap::new();

    for (idx, pool) in pools.iter().enumerate() {
        let key = if pool.base_token_address < pool.quote_token_address {
            (pool.base_token_address, pool.quote_token_address)
        } else {
            (pool.quote_token_address, pool.base_token_address)
        };
        pair_groups.entry(key).or_default().push(idx);
    }

    let mut combos = Vec::new();
    for indices in pair_groups.values() {
        for i in 0..indices.len() {
            for j in (i + 1)..indices.len() {
                debug_assert!(indices[i] < pools.len());
                debug_assert!(indices[j] < pools.len());
                // Çift adını havuz adından çıkar (ilk kısmı at, pair kısmını al)
                let pair_name = pools[indices[i]].name
                    .split('-')
                    .skip(1)
                    .collect::<Vec<_>>()
                    .join("-");
                combos.push(PairCombo {
                    pair_name,
                    pool_a_idx: indices[i],
                    pool_b_idx: indices[j],
                });
            }
        }
    }

    combos
}
