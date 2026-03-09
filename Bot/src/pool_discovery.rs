// ============================================================================
//  POOL DISCOVERY v11.0 — DexScreener API + Otonom Çift Eşleştirme Motoru
//
//  v11.0 Yenilikler:
//  ✓ Token bazlı gruplayma (HashMap<(token0, token1), Vec<Pool>>)
//  ✓ Çapraz-DEX arbitraj çift eşleştirme (aynı token çifti, farklı DEX)
//  ✓ matched_pools.json yapılandırılmış çıktı (serde_json)
//  ✓ build_runtime: JSON → PoolConfig + PairCombo dönüşümü
//
//  v10.0 (korunuyor):
//  ✓ DexScreener API, komisyon filtresi (≤ %0.01), likidite filtresi ($50K+)
// ============================================================================

use eyre::Result;
use serde::{Deserialize, Serialize};
use colored::*;
use std::collections::HashMap;
use alloy::primitives::Address;

use crate::types::{DexType, PoolConfig};

// ─── Sabitler ───
const BASE_WETH_LOWER: &str = "0x4200000000000000000000000000000000000006";
const MATCHED_POOLS_PATH: &str = "matched_pools.json";

// ─────────────────────────────────────────────────────────────────────────────
// DexScreener API Yanıt Yapıları
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct DexScreenerResponse {
    pairs: Option<Vec<DexPair>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DexPair {
    chain_id: String,
    dex_id: String,
    pair_address: String,
    base_token: DexToken,
    quote_token: DexToken,
    liquidity: Option<DexLiquidity>,
    volume: Option<DexVolume>,
    fee_tier: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct DexToken {
    address: String,
    symbol: String,
}

#[derive(Debug, Deserialize)]
struct DexLiquidity {
    usd: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct DexVolume {
    h24: Option<f64>,
}

/// Keşfedilen havuz bilgisi (internal — sadece keşif aşamasında kullanılır)
#[derive(Debug, Clone)]
struct DiscoveredPool {
    address: String,
    dex: String,
    base_token_address: String,
    base_symbol: String,
    quote_token_address: String,
    quote_symbol: String,
    liquidity_usd: f64,
    volume_24h: f64,
    fee_tier: Option<f64>,
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

fn infer_tick_spacing(dex_id: &str, fee_bps: u32) -> i32 {
    let dex_lower = dex_id.to_lowercase();
    if dex_lower.contains("pancakeswap") || dex_lower.contains("pancake") {
        match fee_bps { 1 => 1, 5 => 10, 25 => 50, 100 => 200, _ => 10 }
    } else {
        match fee_bps { 1 => 1, 5 => 10, 30 => 60, 100 => 200, _ => 10 }
    }
}

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

fn infer_dex_type(dex_id: &str) -> DexType {
    let lower = dex_id.to_lowercase();
    if lower.contains("pancakeswap") || lower.contains("pancake") {
        DexType::PancakeSwapV3
    } else if lower.contains("aerodrome") {
        DexType::Aerodrome
    } else {
        DexType::UniswapV3
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DexScreener Keşif
// ─────────────────────────────────────────────────────────────────────────────

async fn discover_base_pools(max_results: usize) -> Result<Vec<DiscoveredPool>> {
    let url = format!(
        "https://api.dexscreener.com/latest/dex/tokens/{}",
        BASE_WETH_LOWER
    );

    eprintln!("  {} DexScreener API sorgulanıyor...", "🔍".cyan());

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| eyre::eyre!("HTTP client hatası: {}", e))?;

    let resp: DexScreenerResponse = client
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| eyre::eyre!("DexScreener API hatası: {}", e))?
        .json()
        .await
        .map_err(|e| eyre::eyre!("JSON parse hatası: {}", e))?;

    let pairs = resp.pairs.unwrap_or_default();

    // Base ağı + minimum likidite filtresi
    let mut discovered: Vec<DiscoveredPool> = pairs
        .into_iter()
        .filter(|p| p.chain_id == "base")
        // ── DEX Beyaz Listesi (V3 ABI uyumlu) ───────────────
        // Uniswap V3 swap() / slot0() ABI’si ile uyumlu DEX’ler.
        // Her DEX'in slot0 struct farkları state_sync.rs ve
        // simulator.rs'de DEX-özel olarak ele alınır.
        // v17.0: Aerodrome Slipstream (CLPool) eklendi —
        //        slot0 6 alan, callback=uniswapV3SwapCallback
        .filter(|p| {
            let dex = p.dex_id.to_lowercase();
            dex.contains("uniswap")
                || dex.contains("pancakeswap")
                || dex.contains("sushiswap")
                || dex.contains("aerodrome")
        })
        .filter(|p| {
            p.liquidity
                .as_ref()
                .and_then(|l| l.usd)
                .unwrap_or(0.0)
                >= 25_000.0
        })
        // v19.0: Minimum 24h hacim filtresi — düşük hacimli havuzlar
        // yeterli volatilite sunmaz, arbitraj fırsatı nadir olur.
        .filter(|p| {
            p.volume
                .as_ref()
                .and_then(|v| v.h24)
                .unwrap_or(0.0)
                >= 10_000.0
        })
        // v19.0: Keşif komisyon filtresi: 0.30 → 1.00 genişletildi.
        // 100 bps'e kadar olan havuzlar keşfedilir, strateji katmanında
        // dinamik net kârlılık hesabı ile süzülür.
        .filter(|p| p.fee_tier.map_or(true, |fee| fee <= 1.00))
        .map(|p| DiscoveredPool {
            address: p.pair_address,
            dex: p.dex_id,
            base_token_address: p.base_token.address,
            base_symbol: p.base_token.symbol,
            quote_token_address: p.quote_token.address,
            quote_symbol: p.quote_token.symbol,
            liquidity_usd: p.liquidity.as_ref().and_then(|l| l.usd).unwrap_or(0.0),
            volume_24h: p.volume.as_ref().and_then(|v| v.h24).unwrap_or(0.0),
            fee_tier: p.fee_tier,
        })
        .collect();

    // Hacme göre azalan sırala
    discovered.sort_by(|a, b| {
        b.volume_24h
            .partial_cmp(&a.volume_24h)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    discovered.truncate(max_results);

    Ok(discovered)
}

// ─────────────────────────────────────────────────────────────────────────────
// Otonom Çift Eşleştirme
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
        // Her DEX'ten en yüksek likiditeli havuzu seç (O(N) tek geçiş)
        let mut dex_best: HashMap<String, &DiscoveredPool> = HashMap::new();
        for pool in group {
            let dex_key = pool.dex.to_lowercase();
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
        // v17.0: En düşük fee'li havuzu öne al, eşit fee'de en yüksek likiditeyi tercih et
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
// CLI Keşif + JSON Çıktı
// ─────────────────────────────────────────────────────────────────────────────

/// Havuzları keşfet, eşleştir ve matched_pools.json olarak yaz
pub async fn cli_discover_pools() -> Result<()> {
    let pools = discover_base_pools(50).await?;

    if pools.is_empty() {
        eprintln!("  {} Hiç havuz bulunamadı.", "⚠️".yellow());
        return Ok(());
    }

    eprintln!("  {} {} havuz keşfedildi, eşleştirme yapılıyor...", "✅".green(), pools.len());

    let matched_pairs = match_arbitrage_pairs(&pools);

    if matched_pairs.is_empty() {
        eprintln!("  {} Arbitraj çifti bulunamadı (en az 2 DEX'te aynı çift gerekli).", "⚠️".yellow());
        return Ok(());
    }

    // Terminal çıktısı
    println!();
    println!("{}", "  ╔═══════════════════════════════════════════════════════════════════╗".cyan().bold());
    println!("{}", "  ║   Otonom Çift Eşleştirme — Base Ağı Arbitraj Havuzları            ║".cyan().bold());
    println!("{}", "  ╠═══════════════════════════════════════════════════════════════════╣".cyan().bold());

    for (i, pair) in matched_pairs.iter().enumerate() {
        println!(
            "  {}  #{} {} ({} havuz)",
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
        version: "11.0".into(),
        chain_id: 8453,
        updated_at: chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string(),
        matched_pairs,
    };

    write_matched_pools_json(&config)?;

    println!(
        "\n  {} matched_pools.json yazıldı ({} çift, {} toplam havuz)",
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
        .map_err(|e| eyre::eyre!("JSON serileştirme hatası: {}", e))?;
    std::fs::write(MATCHED_POOLS_PATH, json)
        .map_err(|e| eyre::eyre!("matched_pools.json yazma hatası: {}", e))?;
    Ok(())
}

/// matched_pools.json dosyasını yükle
pub fn load_matched_pools() -> Result<MatchedPoolsConfig> {
    let content = std::fs::read_to_string(MATCHED_POOLS_PATH)
        .map_err(|e| eyre::eyre!("matched_pools.json okunamadı: {} — Önce `--discover-pools` çalıştırın", e))?;
    let config: MatchedPoolsConfig = serde_json::from_str(&content)
        .map_err(|e| eyre::eyre!("matched_pools.json parse hatası: {}", e))?;
    Ok(config)
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
            .map_err(|e| eyre::eyre!("Geçersiz quote token adresi '{}': {}", pair.quote_token.address, e))?;

        let mut pair_indices: Vec<usize> = Vec::new();

        for pool_entry in &pair.pools {
            let addr_lower = pool_entry.address.to_lowercase();

            let idx = if let Some(&existing_idx) = address_to_idx.get(&addr_lower) {
                existing_idx
            } else {
                let address = pool_entry.address.parse::<Address>()
                    .map_err(|e| eyre::eyre!("Geçersiz havuz adresi '{}': {}", pool_entry.address, e))?;

                let pool_config = PoolConfig {
                    address,
                    name: format!("{}-{}", pool_entry.dex_id, pair.pair_name),
                    fee_bps: pool_entry.fee_bps,
                    fee_fraction: pool_entry.fee_bps as f64 / 10_000.0,
                    token0_decimals: if pair.weth_is_token0 { 18 } else { pair.quote_token.decimals },
                    token1_decimals: if pair.weth_is_token0 { pair.quote_token.decimals } else { 18 },
                    dex: infer_dex_type(&pool_entry.dex_id),
                    token0_is_weth: pair.weth_is_token0,
                    tick_spacing: pool_entry.tick_spacing,
                    quote_token_address,
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
        return Err(eyre::eyre!("matched_pools.json'da geçerli havuz bulunamadı"));
    }

    Ok((all_pools, pair_combos))
}
