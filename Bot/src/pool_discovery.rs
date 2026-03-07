// ============================================================================
//  POOL DISCOVERY v10.0 — DexScreener API ile Dinamik Havuz Keşfi
//
//  Özellikler:
//  ✓ DexScreener API üzerinden Base ağı WETH çiftlerini tara
//  ✓ Hacim ve likiditeye göre sırala → en iyi çapraz-DEX fırsatlarını bul
//  ✓ TARGET_POOLS olarak .env'ye yaz (opsiyonel)
//  ✓ CLI: --discover-pools ile çalıştırılır
// ============================================================================

use eyre::Result;
use serde::Deserialize;
use colored::*;

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
#[allow(dead_code)]
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

/// Keşfedilen havuz bilgisi
#[derive(Debug, Clone)]
pub struct DiscoveredPool {
    pub address: String,
    pub dex: String,
    pub base_symbol: String,
    pub quote_symbol: String,
    pub liquidity_usd: f64,
    pub volume_24h: f64,
    pub fee_tier: Option<f64>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Ana Keşif Fonksiyonu
// ─────────────────────────────────────────────────────────────────────────────

/// Base ağında WETH çiftlerini DexScreener API üzerinden keşfet.
///
/// Filtreleme:
///   - chainId = "base"
///   - Minimum likidite: $50K
///   - DEX: uniswap, aerodrome, pancakeswap vb.
///   - Hacme göre sırala, ilk `max_results` tanesini döndür
pub async fn discover_base_pools(max_results: usize) -> Result<Vec<DiscoveredPool>> {
    // Base WETH adresi
    const BASE_WETH: &str = "0x4200000000000000000000000000000000000006";

    let url = format!(
        "https://api.dexscreener.com/latest/dex/tokens/{}",
        BASE_WETH
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
        .filter(|p| {
            p.liquidity
                .as_ref()
                .and_then(|l| l.usd)
                .unwrap_or(0.0)
                >= 50_000.0
        })
        .map(|p| DiscoveredPool {
            address: p.pair_address,
            dex: p.dex_id,
            base_symbol: p.base_token.symbol,
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

/// Keşfedilen havuzları terminale yazdır ve TARGET_POOLS olarak .env'ye yaz.
pub async fn cli_discover_pools() -> Result<()> {
    let pools = discover_base_pools(20).await?;

    if pools.is_empty() {
        eprintln!("  {} Hiç havuz bulunamadı.", "⚠️".yellow());
        return Ok(());
    }

    println!();
    println!(
        "{}",
        "  ╔═══════════════════════════════════════════════════════════════════╗"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "  ║       DexScreener — Base Ağı WETH Havuzları (Hacme Göre)         ║"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "  ╠═══════════════════════════════════════════════════════════════════╣"
            .cyan()
            .bold()
    );

    for (i, pool) in pools.iter().enumerate() {
        let fee_str = pool
            .fee_tier
            .map(|f| format!("{:.2}%", f))
            .unwrap_or_else(|| "N/A".into());

        println!(
            "  {}  #{:2}  {} | {}/{} | Liq: ${:.0}K | Vol24h: ${:.0}K | Fee: {}",
            "║".cyan(),
            i + 1,
            pool.dex.white().bold(),
            pool.base_symbol,
            pool.quote_symbol,
            pool.liquidity_usd / 1000.0,
            pool.volume_24h / 1000.0,
            fee_str,
        );
        println!(
            "  {}       {}",
            "║".cyan(),
            pool.address.dimmed()
        );
    }

    println!(
        "{}",
        "  ╚═══════════════════════════════════════════════════════════════════╝"
            .cyan()
            .bold()
    );

    // TARGET_POOLS — virgülle ayrılmış adres listesi
    let target_pools: Vec<&str> = pools.iter().map(|p| p.address.as_str()).collect();
    let target_pools_str = target_pools.join(",");

    // .env dosyasına yaz (varsa güncelle, yoksa ekle)
    write_target_pools_to_env(&target_pools_str)?;

    println!(
        "\n  {} TARGET_POOLS .env'ye yazıldı ({} havuz)",
        "✅".green(),
        pools.len()
    );

    Ok(())
}

/// TARGET_POOLS değerini .env dosyasına yaz.
/// Mevcut TARGET_POOLS satırı varsa güncelle, yoksa dosya sonuna ekle.
fn write_target_pools_to_env(pools_csv: &str) -> Result<()> {
    use std::io::Write;

    let env_path = ".env";
    let content = std::fs::read_to_string(env_path).unwrap_or_default();

    let new_line = format!("TARGET_POOLS={}", pools_csv);
    let updated = if content.contains("TARGET_POOLS=") {
        // Mevcut satırı güncelle
        let mut result = String::new();
        for line in content.lines() {
            if line.starts_with("TARGET_POOLS=") {
                result.push_str(&new_line);
            } else {
                result.push_str(line);
            }
            result.push('\n');
        }
        result
    } else {
        // Sonuna ekle
        let mut result = content;
        if !result.ends_with('\n') && !result.is_empty() {
            result.push('\n');
        }
        result.push_str(&new_line);
        result.push('\n');
        result
    };

    let mut file = std::fs::File::create(env_path)
        .map_err(|e| eyre::eyre!(".env yazma hatası: {}", e))?;
    file.write_all(updated.as_bytes())
        .map_err(|e| eyre::eyre!(".env yazma hatası: {}", e))?;

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// TARGET_POOLS Okuma — state_sync entegrasyonu
// ─────────────────────────────────────────────────────────────────────────────

/// .env'den TARGET_POOLS listesini oku (varsa).
/// Format: TARGET_POOLS=0xaddr1,0xaddr2,...
///
/// state_sync::sync_all_pools() tarafından çağrılır — keşfedilen havuzların
/// adresleri bu fonksiyonla alınıp ek havuzlar olarak takip edilebilir.
#[allow(dead_code)]
pub fn load_target_pools_from_env() -> Vec<String> {
    std::env::var("TARGET_POOLS")
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.is_empty() && s.starts_with("0x"))
        .map(|s| s.trim().to_string())
        .collect()
}
