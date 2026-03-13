// ============================================================================
//  ARBITRAJ BOTU v25.0 — "Kuantum Beyin IV"
//  Base Network Çapraz-DEX Arbitraj Sistemi
//
//  v25.0 Yenilikler:
//  ✓ eth_sendRawTransaction (Private RPC) — Flashbots bundle kaldırıldı
//  ✓ Multi-hop arbitraj yürütme (3+ havuzlu triangular/quad)
//  ✓ Gerçek IPC bağlantı desteği (Alloy native IPC)
//  ✓ Otonom Keşif: Factory WSS + Multi-API + Skorlama + GC
//  ✓ Off-Chain TickBitmap Derinlik Simülasyonu (Gerçek Multi-Tick)
//  ✓ Multi-Transport Bağlantı (IPC > WSS > HTTP — Sub-1ms Hedefi)
//  ✓ Base L2 Sequencer Optimizasyonu (FIFO-Aware)
//  ✓ Modüler mimari (types, math, state_sync, simulator, strategy)
// ============================================================================

mod types;
mod math;
mod state_sync;
mod simulator;
mod strategy;
mod key_manager;
mod transport;
mod executor;
mod pool_discovery;
mod discovery_engine;
mod route_engine;
mod json_logger;
mod dust_sweeper;

use types::*;
use state_sync::*;
use simulator::SimulationEngine;
use strategy::*;
use discovery_engine::{DiscoveryConfig, DiscoveryEngine, LivePoolRegistry};

use alloy::primitives::Address;
use alloy::providers::{Provider, ProviderBuilder, WsConnect};
use futures_util::StreamExt;
use futures_util::future::join_all;
use eyre::Result;
use chrono::Local;
use colored::*;
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::collections::HashMap;
use parking_lot::RwLock;
use tokio_util::sync::CancellationToken;

// ─────────────────────────────────────────────────────────────────────────────
// Terminal Çıktı Yardımcıları
// ─────────────────────────────────────────────────────────────────────────────

fn timestamp() -> String {
    Local::now().format("%H:%M:%S%.3f").to_string()
}

fn print_banner(config: &BotConfig) {
    println!();
    println!(
        "{}",
        "╔══════════════════════════════════════════════════════════════════╗"
            .cyan().bold()
    );
    println!(
        "{}",
        "║       ARBITRAGE BOT v25.0 — Quantum Brain IV                    ║"
            .cyan().bold()
    );
    println!(
        "{}",
        "║    Base Network Cross-DEX Arbitrage System                       ║"
            .cyan().bold()
    );
    println!(
        "{}",
        "╠══════════════════════════════════════════════════════════════════╣"
            .cyan().bold()
    );
    println!(
        "{}",
        "║  [v25] Autonomous Discovery: Factory WSS + Multi-API + Scoring + GC ║"
            .cyan()
    );
    println!(
        "{}",
        "║  [v9] Executor/Admin Role Separation + Deadline Block             ║"
            .cyan()
    );
    println!(
        "{}",
        "║  [v9] Encrypted Key Management (AES-256-GCM + PBKDF2)             ║"
            .cyan()
    );
    println!(
        "{}",
        "║  [v9] Dynamic Bribe/Priority Fee + 134-Byte Calldata              ║"
            .cyan()
    );
    println!(
        "{}",
        "║  [v6] TickBitmap + Multi-Tick Depth + REVM Simulation              ║"
            .cyan()
    );
    println!(
        "{}",
        "║  [v5] State Sync + Newton-Raphson + Multi-Transport            ║"
            .cyan()
    );
    println!(
        "{}",
        "╚══════════════════════════════════════════════════════════════════╝"
            .cyan().bold()
    );
    println!();
    println!("  {} Engine         : {}", "▸".cyan(), "Rust + Alloy + REVM (Zero Latency)".white());
    println!("  {} Network        : {}", "▸".cyan(), format!("Base Network (Chain ID: {})", config.chain_id).white());
    println!("  {} Transport      : {}", "▸".cyan(), format!("{:?} (Priority: IPC > WSS > HTTP)", config.transport_mode).white());
    println!("  {} Strategy       : {}", "▸".cyan(), "Cross-DEX Spread Arbitrage (Uniswap V3 + Aerodrome)".white());
    println!("  {} Depth          : {}", "▸".cyan(), format!("TickBitmap (±{} tick range, max {}blk age)", config.tick_bitmap_range, config.tick_bitmap_max_age_blocks).white());
    println!("  {} Calldata       : {}", "▸".cyan(), format!("134 byte compact (deadline: +{} block)", config.deadline_blocks).white());
    println!("  {} Bribe          : {}", "▸".cyan(), format!("Dynamic %{:.0} profit → priority fee", config.bribe_pct * 100.0).white());
    println!("  {} Key Mgmt       : {}", "▸".cyan(), if config.key_manager_active { "Encrypted Keystore (AES-256-GCM)".green().to_string() } else if config.private_key.is_some() { "Env Var (UNSAFE)".yellow().to_string() } else { "None".red().to_string() });
    println!("  {} Flash Loan     : {}", "▸".cyan(), format!("Aave V3 ({:.2}% Fee)", config.flash_loan_fee_bps / 100.0).white());
    println!("  {} Max Trade      : {}", "▸".cyan(), format!("{:.1} WETH", config.max_trade_size_weth).white());
    println!("  {} Min Net Profit : {}", "▸".cyan(), format!("{:.6} WETH", config.min_net_profit_weth).white());
    println!(
        "  {} Start Time     : {}",
        "▸".cyan(),
        Local::now().format("%Y-%m-%d %H:%M:%S").to_string().yellow()
    );
    println!(
        "  {} Mode           : {}",
        "▸".cyan(),
        if config.execution_enabled() {
            "LIVE (Contract Execution Active)".green().bold().to_string()
        } else if config.shadow_mode() {
            "SHADOW MODE (Dry Run — logging to shadow_analytics.jsonl)".yellow().bold().to_string()
        } else {
            "OBSERVE (Watch Only)".yellow().bold().to_string()
        }
    );
    println!();
}

fn print_pool_header(pools: &[PoolConfig], states: &[SharedPoolState]) {
    println!("{}", "  ┌──────────────────────────────────────────────────────────────┐".dimmed());
    println!("  {} {}", "│".dimmed(), "Monitored Pools:".white().bold());
    for (i, p) in pools.iter().enumerate() {
        let icon = if i == 0 { "🔵" } else { "🟣" };
        let fee_display = if i < states.len() {
            states[i].read().live_fee_bps
                .map(|b| b as f64 / 100.0)
                .unwrap_or(p.fee_bps as f64 / 100.0)
        } else {
            p.fee_bps as f64 / 100.0
        };
        println!(
            "  {}   {} {} ({} — Fee: %{:.2})",
            "│".dimmed(),
            icon,
            p.name,
            p.dex,
            fee_display
        );
        println!("  {}     {}", "│".dimmed(), format!("{}", p.address).dimmed());
    }
    println!("{}", "  └──────────────────────────────────────────────────────────────┘".dimmed());
    println!();
}

fn print_block_update(
    block_number: u64,
    pools: &[PoolConfig],
    states: &[SharedPoolState],
    sync_ms: u128,
) {
    let mut pool_info = String::new();
    for (i, (config, state_lock)) in pools.iter().zip(states.iter()).enumerate() {
        let state = state_lock.read();
        if state.is_active() {
            if i > 0 {
                pool_info.push_str(" | ");
            }
            let short_name = if config.name.len() > 12 {
                &config.name[..12]
            } else {
                &config.name
            };
            pool_info.push_str(&format!(
                "{}={:.6}Q",
                short_name,
                state.eth_price_usd,
            ));
        }
    }

    println!(
        "  {} [{}] Block #{} | {} | Sync: {}ms",
        "🧱".blue(),
        timestamp().dimmed(),
        format!("{}", block_number).white().bold(),
        pool_info,
        sync_ms,
    );

    // JSON structured log: block processed
    json_logger::log_block(block_number, sync_ms, pools.len());
}

fn print_spread_info(pools: &[PoolConfig], states: &[SharedPoolState]) {
    if states.len() < 2 {
        return;
    }

    let state_a = states[0].read();
    let state_b = states[1].read();

    if !state_a.is_active() || !state_b.is_active() {
        return;
    }

    let spread = (state_a.eth_price_usd - state_b.eth_price_usd).abs();
    let min_price = state_a.eth_price_usd.min(state_b.eth_price_usd);
    let spread_pct = if min_price > 0.0 {
        (spread / min_price) * 100.0
    } else {
        0.0
    };

    if spread_pct > 0.001 {
        let direction = if state_a.eth_price_usd < state_b.eth_price_usd {
            format!("{} → {}", pools[0].name, pools[1].name)
        } else {
            format!("{} → {}", pools[1].name, pools[0].name)
        };

        if spread_pct > 0.05 {
            println!(
                "     {} Spread: {:.4}% ({:.6}Q) | {} BUY→SELL",
                "📊".yellow(), spread_pct, spread, direction,
            );
        } else {
            println!(
                "     📊 Spread: {:.4}% ({:.6}Q) | {}", spread_pct, spread, direction,
            );
        }
    }
}

fn print_stats_summary(stats: &ArbitrageStats, states: &[SharedPoolState], pools: &[PoolConfig], pair_combos: &[pool_discovery::PairCombo]) {
    println!();
    println!("{}", "  ┌───── SESSION STATISTICS (v16.2) ──────────────────────────────┐".yellow());
    println!("  {}  Uptime               : {}", "│".yellow(), stats.uptime_str().white().bold());
    println!("  {}  Blocks Processed     : {}", "│".yellow(), format!("{}", stats.total_blocks_processed).white());
    println!("  {}  Opportunities Detected: {}", "│".yellow(), format!("{}", stats.total_opportunities).white());
    println!(
        "  {}  Net Profitable       : {}",
        "│".yellow(),
        if stats.profitable_opportunities > 0 {
            format!("{}", stats.profitable_opportunities).green().bold().to_string()
        } else {
            format!("{}", stats.profitable_opportunities).dimmed().to_string()
        }
    );
    println!("  {}  Failed Simulations   : {}", "│".yellow(), stats.failed_simulations);
    println!(
        "  {}  Executed Trades      : {}",
        "│".yellow(),
        if stats.executed_trades > 0 {
            format!("{}", stats.executed_trades).green().bold().to_string()
        } else {
            format!("{}", stats.executed_trades).dimmed().to_string()
        }
    );
    println!("  {}  Max Spread           : {:.4}%", "│".yellow(), stats.max_spread_pct);
    println!("  {}  Max Profit (single)  : {:.6} WETH", "│".yellow(), stats.max_profit_weth);
    println!("  {}  Total Pot. Profit    : {:.6} WETH", "│".yellow(), stats.total_potential_profit);

    // v11.0: Fee & break-even — tüm çiftler
    println!("  {} ─── Fee & Economic Analysis ───────────────", "│".yellow());
    let mut min_total_fee_pct = f64::MAX;
    for combo in pair_combos {
        if combo.pool_a_idx < pools.len() && combo.pool_b_idx < pools.len() {
            let fee_a = if combo.pool_a_idx < states.len() {
                states[combo.pool_a_idx].read().live_fee_bps
                    .map(|b| b as f64 / 10_000.0)
                    .unwrap_or(pools[combo.pool_a_idx].fee_fraction)
            } else {
                pools[combo.pool_a_idx].fee_fraction
            };
            let fee_b = if combo.pool_b_idx < states.len() {
                states[combo.pool_b_idx].read().live_fee_bps
                    .map(|b| b as f64 / 10_000.0)
                    .unwrap_or(pools[combo.pool_b_idx].fee_fraction)
            } else {
                pools[combo.pool_b_idx].fee_fraction
            };
            let total = (fee_a + fee_b) * 100.0;
            if total < min_total_fee_pct { min_total_fee_pct = total; }
            println!("  {}  {} : {:.2}% + {:.2}% = {:.2}%",
                "│".yellow(), combo.pair_name,
                fee_a * 100.0, fee_b * 100.0, total,
            );
        }
    }
    if min_total_fee_pct < f64::MAX {
        let profitable = stats.max_spread_pct > min_total_fee_pct;
        if profitable {
            println!("  {}  Status               : {} (spread > fee)", "│".yellow(), "POTENTIALLY PROFITABLE".green().bold());
        } else {
            println!("  {}  Status               : {} (spread {:.4}% < min fee {:.2}%)", "│".yellow(), "UNPROFITABLE".red().bold(), stats.max_spread_pct, min_total_fee_pct);
        }
    }

    // v6.0: Gecikme istatistikleri
    println!("  {} ─── Latency (State Sync) ─────────────────", "│".yellow());
    println!("  {}  Avg Latency          : {:.1}ms", "│".yellow(), stats.avg_block_latency_ms);
    println!("  {}  Min Latency          : {:.1}ms", "│".yellow(), stats.min_block_latency_ms);
    println!("  {}  Max Latency          : {:.1}ms", "│".yellow(), stats.max_block_latency_ms);
    println!("  {}  Latency Spikes       : {} times", "│".yellow(), stats.latency_spikes);
    println!("  {}  TickBitmap Sync       : {} times", "│".yellow(), stats.tick_bitmap_syncs);

    for (i, state_lock) in states.iter().enumerate() {
        let state = state_lock.read();
        if state.is_active() {
            let bitmap_info = if let Some(ref bm) = state.tick_bitmap {
                format!(" | Bitmap: {} tick", bm.ticks.len())
            } else {
                " | Bitmap: NONE".to_string()
            };
            println!(
                "  {}  Pool {} Price        : {:.6} Q (tick: {}){}",
                "│".yellow(), i + 1, state.eth_price_usd, state.tick, bitmap_info,
            );
        }
    }

    println!("{}", "  └──────────────────────────────────────────────────────────────┘".yellow());
    println!();

    // JSON structured log: session statistics snapshot
    json_logger::log_stats(
        &stats.uptime_str(),
        stats.total_blocks_processed,
        stats.total_opportunities,
        stats.profitable_opportunities,
        stats.executed_trades,
        stats.total_potential_profit,
        stats.avg_block_latency_ms,
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// GÖREV 3: Kendi Kendini Onaran .env Şablonu — Fail-Safe Generator
// ─────────────────────────────────────────────────────────────────────────────

/// .env dosyası yoksa standart HFT altyapı şablonunu diske yazar ve
/// kullanıcıya RPC URL'lerini girmesini söyleyip zarifçe (graceful) kapanır.
fn generate_default_env_and_exit() -> ! {
    let template = r#"# ═══════════════════════════════════════════════════════════════════════════════
#  Quantum Brain III v9.0 — Auto-Generated .env Template
#
#  This file was auto-generated by the bot.
#  Please fill in the RPC URLs below with your Alchemy/Infura/QuickNode
#  credentials and restart the bot.
# ═══════════════════════════════════════════════════════════════════════════════

# ─── RPC Connections (REQUIRED — Alchemy / Infura / QuickNode) ───
RPC_WSS_URL=
RPC_HTTP_URL=
RPC_WSS_URL_BACKUP=
RPC_WSS_URL_2=
RPC_WSS_URL_3=
RPC_IPC_PATH=
TRANSPORT_MODE=auto

# ─── Chain Config (Base Mainnet) ───
CHAIN_ID=8453

# ─── Wallet and Contract ───
PRIVATE_KEY=
KEYSTORE_PATH=
ARBITRAGE_CONTRACT_ADDRESS=

# ─── MEV Protection (optional) ───
PRIVATE_RPC_URL=

# ─── Cost and Strategy (in WETH) ───
GAS_COST_FALLBACK_WETH=0.00005
FLASH_LOAN_FEE_BPS=0.0
MIN_NET_PROFIT_WETH=0.00003
MAX_TRADE_SIZE_WETH=5.0
MAX_STALENESS_MS=3000
STATS_INTERVAL=10
MAX_RETRIES=0

# ─── Pool Fee Filter ───
MAX_POOL_FEE_BPS=100

# ─── TickBitmap Depth Settings ───
TICK_BITMAP_RANGE=100
TICK_BITMAP_MAX_AGE_BLOCKS=5

# ─── Shadow Mode ───
EXECUTION_ENABLED=false

# ─── RPC Failover & Latency Settings ───
LATENCY_SPIKE_THRESHOLD_MS=200

# ─── MEV & TX Settings ───
DEADLINE_BLOCKS=2
BRIBE_PCT=0.25
CIRCUIT_BREAKER_THRESHOLD=3

# ─── Admin (optional) ───
ADMIN_ADDRESS=
"#;

    match std::fs::write(".env", template) {
        Ok(_) => {
            println!();
            println!(
                "╔══════════════════════════════════════════════════════════════════╗"
            );
            println!(
                "║  .env file not found — default template generated.             ║"
            );
            println!(
                "║                                                                  ║"
            );
            println!(
                "║  Please open the .env file,                                      ║"
            );
            println!(
                "║  fill in RPC_WSS_URL and RPC_HTTP_URL fields                     ║"
            );
            println!(
                "║  and restart the bot.                                            ║"
            );
            println!(
                "╚══════════════════════════════════════════════════════════════════╝"
            );
            println!();
        }
        Err(e) => {
            eprintln!("  ERROR: Could not write .env template: {}", e);
        }
    }

    std::process::exit(0)
}

// ─────────────────────────────────────────────────────────────────────────────
// ANA GİRİŞ NOKTASI — Yeniden Bağlanma Döngüsü
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    // ═══ GÖREV 3: Kendi Kendini Onaran .env Şablonu ═══
    // .env dosyası yoksa standart HFT şablonu oluştur ve zarifçe kapat.
    if dotenvy::dotenv().is_err() {
        if !std::path::Path::new(".env").exists() {
            generate_default_env_and_exit();
        }
        // .env var ama parse hatası olabilir — devam et, env::var fallback'leri yeterli
        eprintln!(
            "  {} .env file was read but some lines could not be parsed — defaults will be used.",
            "⚠️".yellow()
        );
    }

    // ═══ CLI: --encrypt-key argümanı ile keystore oluşturma ═══
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--encrypt-key") {
        return key_manager::KeyManager::cli_encrypt_key();
    }

    // ═══ CLI: --discover-pools ile DexScreener havuz keşfi ═══
    if args.iter().any(|a| a == "--discover-pools") {
        return pool_discovery::cli_discover_pools().await;
    }

    // ═══ CLI: --sweep-dust ile dust token temizliği ═══
    if args.iter().any(|a| a == "--sweep-dust") {
        let execute = args.iter().any(|a| a == "--execute");
        return dust_sweeper::run_sweep(execute).await;
    }

    // Yapılandırmayı oku
    let mut config = BotConfig::from_env()?;

    // ═══ CLI: --mode shadow|live ile mod geçersiz kılma ═══
    if let Some(pos) = args.iter().position(|a| a == "--mode") {
        if let Some(mode) = args.get(pos + 1) {
            match mode.to_lowercase().as_str() {
                "shadow" => {
                    config.execution_enabled_flag = false;
                    println!(
                        "  {} CLI: --mode shadow → Shadow mode forced",
                        "👻".yellow()
                    );
                }
                "live" => {
                    config.execution_enabled_flag = true;
                    println!(
                        "  {} CLI: --mode live → Live mode forced",
                        "🚀".green()
                    );
                }
                other => {
                    return Err(eyre::eyre!(
                        "Invalid mode: '{}'. Usage: --mode shadow|live",
                        other
                    ));
                }
            }
        }
    }

    // ═══ GÖREV 2: Auto-Bootstrap — Her başlangıçta havuz keşfi (v32.0) ═══
    // ═══ v29.0: CORE POOLS — Statik beyaz liste öncelikli ═══
    let matched_cfg = if let Some(core_cfg) = pool_discovery::load_core_pools() {
        eprintln!("  {} core_pools.json found — skipping auto-discovery.", "⚙️".cyan());
        core_cfg
    } else {
        eprintln!("  {} Auto pool discovery (Holy Trinity) starting...", "🔍".cyan());
        pool_discovery::cli_discover_pools().await?;
        pool_discovery::load_matched_pools()?
    };
    let (pools_initial, pair_combos_initial) = pool_discovery::build_runtime(&matched_cfg)?;
    // v25.0: Havuz listeleri artık mutable — hot-reload için
    let mut pools = pools_initial;
    let mut pair_combos = pair_combos_initial;

    // ═══ v21.0: PRIVATE RPC ZORUNLULUĞU ═══
    // v21.0: Public mempool gönderimi tamamen kaldırıldı.
    // PRIVATE_RPC_URL olmadan bot işlem gönderemez.
    // Shadow mode'da Private RPC gerekmez.
    if config.execution_enabled() && config.private_rpc_url.is_none() {
        return Err(eyre::eyre!(
            "PRIVATE_RPC_URL not defined! Since v21.0 public mempool submission has been \
             removed. Add PRIVATE_RPC_URL=https://... to .env or \
             use shadow mode with EXECUTION_ENABLED=false."
        ));
    }

    // ═══ v11.0: TOKEN WHITELIST DOĞRULAMA (tüm çiftler) ═══
    {
        let wl = crate::types::token_whitelist();
        if !wl.contains(&config.weth_address) {
            return Err(eyre::eyre!("WETH address ({}) NOT in whitelist!", config.weth_address));
        }
        for pool in &pools {
            if !wl.contains(&pool.quote_token_address) {
                return Err(eyre::eyre!("Quote token {} NOT in whitelist!", pool.quote_token_address));
            }
        }
    }
    println!(
        "  {} Token Whitelist: All token addresses verified ({} pools)",
        "✅".green(), pools.len()
    );

    // ═══ v9.0: KEY MANAGER BAŞLATMA ═══
    // Öncelik: 1) Şifreli keystore → 2) Env var (uyarıyla) → 3) Key yok
    let key_manager = key_manager::KeyManager::auto_load()?;
    if key_manager.has_key() {
        config.key_manager_active = true;
        // Keystore'dan gelen key'i config.private_key'e de aktar (geriye uyumluluk)
        if config.private_key.is_none() {
            config.private_key = key_manager.private_key().map(|k: &str| k.to_string());
        }
        println!(
            "  {} Key Mgmt: {}",
            "🔐".green(),
            key_manager.source()
        );
    } else {
        println!(
            "  {} Key Mgmt: No key loaded (observe mode)",
            "ℹ️".blue()
        );
    }

    // Banner göster
    print_banner(&config);

    // Yeniden bağlanma döngüsü
    let mut retry_count: u32 = 0;

    loop {
        if retry_count > 0 {
            println!(
                "  {} Reconnection attempt #{}",
                "🔄".yellow(), retry_count
            );
        }

        match run_bot(&config, &mut pools, &mut pair_combos).await {
            Ok(_) => {
                println!(
                    "\n  {} Connection lost. Reconnecting...",
                    "⚠️".yellow()
                );
            }
            Err(e) => {
                // CancellationToken .cancel() eski listener'ları temizler.
                // run_bot döndüğünde token scope'u biter, yeni döngüde
                // yeni token üretilir.
                println!(
                    "\n  {} Error: {:#}",
                    "❌".red(), e
                );
            }
        }

        retry_count += 1;

        if config.max_retries > 0 && retry_count >= config.max_retries {
            println!(
                "  {} Maximum retries ({}) exceeded. Bot shutting down.",
                "🛑".red(), config.max_retries
            );
            return Err(eyre::eyre!("Maximum reconnection attempts exceeded"));
        }

        // v13.0: Akıllı reconnect — ilk 3 deneme hızlı, sonra exponential backoff
        // İlk kopmalarda hızlı geri dönüş, uzun süren kesintilerde rate-limit koruması.
        // v23.0 (D-4): Jitter eklendi — thundering herd / rate-limit koruması
        let delay_ms = if retry_count <= 3 {
            100u64 // İlk 3 deneme: 100ms (agresif)
        } else {
            // Exponential backoff: 200ms → 400ms → 800ms → ... → max 30s
            let exp_delay = 100u64 * (1u64 << (retry_count - 3).min(8));
            exp_delay.min(30_000) // v22.1: Üst sınır: 30 saniye (eski: 10s)
        };
        // v23.0 (D-4): Random jitter eklendi (0..%50 delay veya max 2s)
        let jitter_range = (delay_ms / 2).clamp(1, 2000);
        let jitter = rand::random::<u64>() % jitter_range;
        let delay_ms = delay_ms + jitter;
        println!(
            "  {} Reconnecting in {}ms... (attempt #{})",
            "⚡".yellow(), delay_ms, retry_count
        );
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BOT MOTORU — Blok Dinle → State Sync → Fırsat Tara → Simüle → Yürüt
// ─────────────────────────────────────────────────────────────────────────────

async fn run_bot(config: &BotConfig, pools: &mut Vec<PoolConfig>, pair_combos: &mut Vec<pool_discovery::PairCombo>) -> Result<()> {
    // ══════════════ CANCELLATION TOKEN (v11.0: Zombi Thread Önleme) ══════════════
    // Her run_bot çağrısında yeni bir CancellationToken üretilir.
    // Arka plan listener'larına (pending_tx, swap_event) paslanır.
    // run_bot hata ile çıktığında token.cancel() çağrılarak
    // tüm zombi WebSocket dinleyicileri temiz biçimde sonlandırılır.
    let cancel_token = CancellationToken::new();

    // Scope guard: fonksiyon herhangi bir şekilde çıkarsa token'ı iptal et
    let _cancel_guard = cancel_token.clone().drop_guard();

    // ══════════════ COOL-DOWN BLACKLIST (v11.0: Circuit Breaker Refactor) ══════════════
    // PairId (combo index) → blacklist_until_block
    // Bir çift 3 ardışık kez başarısız olursa, current_block + 100 bloğa kadar engellenir.
    // Bot çalışmaya devam eder, sadece o çifti atlar.
    let mut pair_cooldown: HashMap<usize, u64> = HashMap::new();
    // Per-pair ardışık hata sayacı: combo_index → consecutive_failures
    let mut pair_failures: HashMap<usize, u32> = HashMap::new();

    // ══════════════ MULTI-TRANSPORT BAĞLANTI (v10.0: RpcPool) ══════════════
    // IPC öncelikli, Round-Robin WSS fallback, arka plan sağlık kontrolü
    println!("  {} Establishing transport connection ({:?} mode)...", "⏳".yellow(), config.transport_mode);
    let connect_start = Instant::now();

    // RPC Pool için WSS URL listesi oluştur
    let mut ws_urls = vec![config.rpc_wss_url.clone()];
    if let Some(ref backup) = config.rpc_wss_url_backup {
        ws_urls.push(backup.clone());
    }
    ws_urls.extend(config.rpc_wss_url_extra.iter().cloned());

    // RpcPool oluştur ve bağlan
    let mut rpc_pool = transport::RpcPool::new(
        config.rpc_ipc_path.clone(),
        &ws_urls,
    );
    rpc_pool.connect_all().await?;
    let rpc_pool = Arc::new(rpc_pool);

    // Arka plan sağlık kontrolü başlat (2s aralıkla node yoklama)
    rpc_pool.spawn_health_checker();

    println!(
        "  {} RpcPool ready: {} | Healthy nodes: {}",
        "✅".green(),
        rpc_pool.transport_info().cyan(),
        rpc_pool.healthy_node_count(),
    );

    // Primary provider al (ana döngü için)
    let provider = rpc_pool.get_provider().await?;
    let active_transport = rpc_pool.transport_info();

    let total_connect_ms = connect_start.elapsed().as_millis();

    // ══════════════ MEV EXECUTOR (v21.0) ══════════════
    let mev_executor = Arc::new(executor::MevExecutor::new(
        config.private_rpc_url.clone(),
        config.rpc_wss_url.clone(),
        config.bribe_pct,
    ));
    if config.private_rpc_url.is_some() {
        println!(
            "  {} MEV Protection: {} (eth_sendRawTransaction active)",
            "🛡️".green(),
            "ACTIVE".green().bold()
        );
    } else {
        println!(
            "  {} MEV Protection: {} (define PRIVATE_RPC_URL)",
            "⚠️".yellow(),
            "DISABLED".yellow().bold()
        );
    }

    // Son blok
    let block = provider.get_block_number().await?;
    println!(
        "  {} Current block: #{} | Transport: {} | Connection: {}ms",
        "🧱".blue(),
        format!("{}", block).white().bold(),
        active_transport.cyan(),
        total_connect_ms,
    );

    // ══════════════ PAYLAŞIMLI DURUM ══════════════
    let mut states: Vec<SharedPoolState> = pools.iter()
        .map(|_| Arc::new(RwLock::new(PoolState::default())))
        .collect();

    // ══════════════ İLK SENKRONİZASYON ══════════════
    println!("\n  {} Performing initial state sync...", "🔄".yellow());

    // Bytecode önbelleğe al (bir kez — REVM için)
    let bytecode_results = cache_all_bytecodes(&provider, pools, &states).await;
    for (i, result) in bytecode_results.iter().enumerate() {
        match result {
            Ok(_) => println!("  {}   {} bytecode cached", "✅".green(), pools[i].name),
            Err(e) => println!("  {}   {} bytecode error: {}", "⚠️".yellow(), pools[i].name, e),
        }
    }

    // ══════════════ HAVUZ SAĞLAMLIK KONTROLÜ (Pool Sanity Check) ══════════════
    // v10.0: Başlangıçta tüm havuzları on-chain doğrula. Geçersiz havuzlar
    // (execution reverted, slot0/liquidity okunamayan) listeden çıkarılır.
    // Bu, runtime'da "error code 3: execution reverted" hatalarını önler.
    println!("\n  {} Performing pool health check ({} pools)...", "🔍".yellow(), pools.len());
    let invalid_pool_indices = validate_pools(&provider, pools).await;
    if !invalid_pool_indices.is_empty() {
        println!(
            "  {} {} invalid pools detected — removing from list",
            "⚠️".yellow(), invalid_pool_indices.len(),
        );
        // Büyükten küçüğe sırala — indeks kayması olmasın
        let mut sorted_invalid = invalid_pool_indices;
        sorted_invalid.sort_unstable_by(|a, b| b.cmp(a));
        for &idx in &sorted_invalid {
            println!(
                "  {}   Removed: {} ({})",
                "🗑️".red(), pools[idx].name, pools[idx].address,
            );
            pools.remove(idx);
            states.remove(idx);
        }
        // pair_combos'u geçerli indekslerle yeniden oluştur
        *pair_combos = pool_discovery::rebuild_pair_combos(pools);
        for combo in pair_combos.iter() {
            if combo.pool_a_idx >= pools.len() || combo.pool_b_idx >= pools.len() {
                return Err(eyre::eyre!(
                    "Invalid pair combo index generated after pool validation: {} -> ({}, {}) / pool_count={} ",
                    combo.pair_name,
                    combo.pool_a_idx,
                    combo.pool_b_idx,
                    pools.len()
                ));
            }
        }
        println!(
            "  {} Pool list updated: {} valid pools remaining",
            "✅".green(), pools.len(),
        );
    } else {
        println!(
            "  {} All pools validated — {} pools valid",
            "✅".green(), pools.len(),
        );
    }

    // İlk state sync
    let sync_results = sync_all_pools(&provider, pools, &states, block).await;
    for (i, result) in sync_results.iter().enumerate() {
        match result {
            Ok(_) => {
                let state = states[i].read();
                let fee_info = match state.live_fee_bps {
                    Some(bps) => format!("Fee: {}bps ({:.2}%)", bps, bps as f64 / 100.0),
                    None => format!("Fee: N/A (config: {}bps)", pools[i].fee_bps),
                };
                println!(
                    "  {}   {} → {:.6} Q | Tick: {} | Liquidity: {:.2e} | {}",
                    "✅".green(),
                    pools[i].name,
                    state.eth_price_usd,
                    state.tick,
                    state.liquidity_f64,
                    fee_info,
                );
            }
            Err(e) => println!("  {}   {} state error: {}", "❌".red(), pools[i].name, e),
        }
    }

    // ══════════════ İLK TİCKBİTMAP SENKRONİZASYONU ══════════════
    println!("\n  {} Fetching TickBitmap depth map (±{} tick)...", "🗺️".yellow(), config.tick_bitmap_range);
    let bitmap_start = Instant::now();
    let bitmap_results = sync_all_tick_bitmaps(
        &provider, pools, &states, block, config.tick_bitmap_range,
    ).await;
    let bitmap_ms = bitmap_start.elapsed().as_millis();

    for (i, result) in bitmap_results.iter().enumerate() {
        match result {
            Ok(_) => {
                let state = states[i].read();
                if let Some(ref bm) = state.tick_bitmap {
                    println!(
                        "  {}   {} → {} inicialize tick, {} word | {}ms",
                        "✅".green(),
                        pools[i].name,
                        bm.ticks.len(),
                        bm.words.len(),
                        bm.sync_duration_us / 1000,
                    );
                }
            }
            Err(e) => println!("  {}   {} bitmap error: {}", "⚠️".yellow(), pools[i].name, e),
        }
    }
    println!("  {} TickBitmap total time: {}ms", "🗺️".cyan(), bitmap_ms);

    // State sync tamamlandı — havuz başlığını canlı fee'lerle göster
    print_pool_header(pools, &states);

    // ══════════════ REVM SİMÜLASYON MOTORU ══════════════
    let mut sim_engine = SimulationEngine::new();
    sim_engine.set_chain_id(config.chain_id);
    sim_engine.cache_bytecodes(pools, &states);

    // v22.1: Kontrat bytecode'unu zincirden al — simülasyonda gerçek kontrat çalışsın
    // v24.0: Zincirden alınamazsa Foundry artifact'ten yükle (local fallback)
    if let Some(contract_addr) = config.contract_address {
        let mut bytecode_loaded = false;
        match provider.get_code_at(contract_addr).await {
            Ok(code) if !code.is_empty() => {
                println!("  {} Contract bytecode loaded ({} bytes — from chain)", "✅".green(), code.len());
                sim_engine.set_contract_bytecode(code.to_vec());
                bytecode_loaded = true;
            }
            Ok(_) => {
                eprintln!("  {} Contract bytecode empty — may not be deployed, searching local artifact...", "⚠️".yellow());
            }
            Err(e) => {
                eprintln!("  {} Contract bytecode fetch failed: {} — searching local artifact...", "⚠️".yellow(), e);
            }
        }
        // v24.0: Fallback — Foundry out/ veya Contract/out/ dizininden derlenmiş bytecode yükle
        if !bytecode_loaded {
            let artifact_paths = [
                "../Contract/out/Arbitraj.sol/ArbitrajBotu.json",
                "Contract/out/Arbitraj.sol/ArbitrajBotu.json",
            ];
            for path in &artifact_paths {
                if let Ok(content) = std::fs::read_to_string(path) {
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                        if let Some(deployed) = json.get("deployedBytecode")
                            .and_then(|v| v.get("object"))
                            .and_then(|v| v.as_str())
                        {
                            let hex_str = deployed.strip_prefix("0x").unwrap_or(deployed);
                            if let Ok(bytes) = alloy::primitives::hex::decode(hex_str) {
                                if !bytes.is_empty() {
                                    println!("  {} Contract bytecode loaded ({} bytes — local artifact: {})", "✅".green(), bytes.len(), path);
                                    sim_engine.set_contract_bytecode(bytes);
                                    bytecode_loaded = true;
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            if !bytecode_loaded {
                eprintln!("  {} Contract bytecode not found — REVM simulation will use mathematical fallback", "⚠️".yellow());
            }
        }
    }

    // v10.0: Singleton base_db — bytecode bir kez yüklenir, sonra her blokta klonlanır
    {
        let caller_addr = config.private_key.as_ref()
            .and_then(|pk| pk.parse::<alloy::signers::local::PrivateKeySigner>().ok())
            .map(|signer| signer.address())
            .unwrap_or_default();
        let contract_addr = config.contract_address.unwrap_or_default();
        sim_engine.initialize_base_db(pools, &states, caller_addr, contract_addr);
        println!("\n  {} REVM simulation engine ready (Singleton base_db)", "✅".green());
    }

    // ══════════════ ATOMİK NONCE YÖNETİCİSİ ══════════════
    let executor_address: Option<Address> = config.private_key.as_ref()
        .and_then(|pk| pk.parse::<alloy::signers::local::PrivateKeySigner>().ok())
        .map(|signer| signer.address());

    let nonce_manager = if let Some(address) = executor_address {
        println!("  {} Reading nonce ({})...", "🔢".yellow(), address);
        match provider.get_transaction_count(address).await {
            Ok(nonce) => {
                println!("  {} Initial nonce: {} (from RPC)", "✅".green(), nonce);
                Arc::new(NonceManager::new(nonce))
            }
            Err(e) => {
                println!("  {} Nonce read failed, starting from 0: {}", "⚠️".yellow(), e);
                Arc::new(NonceManager::new(0))
            }
        }
    } else {
        Arc::new(NonceManager::new(0))
    };

    // Execution modu
    if config.execution_enabled() {
        println!(
            "  {} Contract execution: {} (Address: {})",
            "🚀".green(),
            "ACTIVE".green().bold(),
            config.contract_address
                .expect("BUG: execution_enabled() true but contract_address None")
        );
    } else {
        println!(
            "  {} Contract execution: {} (Observe only)",
            "ℹ️".blue(),
            "DISABLED".yellow().bold()
        );
    }

    // ══════════════ BAŞLANGIÇ WHİTELİST SYNC ══════════════
    // Tüm core havuzları on-chain whiteliste ekle (executorBatchAddPools).
    // İdempotent — zaten whitelistte olan havuzlar tekrar eklense sorun olmaz.
    if config.execution_enabled() {
        let all_pool_addrs: Vec<Address> = pools.iter().map(|p| p.address).collect();
        if !all_pool_addrs.is_empty() {
            let calldata = crate::executor::encode_whitelist_calldata(&all_pool_addrs);
            if let (Some(ref pk), Some(contract_addr)) = (&config.private_key, config.contract_address) {
                let startup_base_fee = provider
                    .get_block_by_number(
                        alloy::eips::BlockNumberOrTag::Latest,
                    )
                    .await
                    .ok()
                    .flatten()
                    .and_then(|b| b.header.base_fee_per_gas)
                    .unwrap_or(1_000_000_000) as u64;

                let nonce = nonce_manager.get_and_increment();
                match whitelist_pools_on_chain(
                    Arc::clone(&mev_executor),
                    pk.clone(),
                    contract_addr,
                    calldata,
                    nonce,
                    Arc::clone(&nonce_manager),
                    startup_base_fee,
                ).await {
                    Ok(_) => println!(
                        "  {} [Whitelist] {} pools added to on-chain whitelist (startup sync)",
                        "✅".green(), all_pool_addrs.len(),
                    ),
                    Err(e) => eprintln!(
                        "  {} [Whitelist] Startup whitelist error: {} — admin must add manually",
                        "⚠️".yellow(), e,
                    ),
                }
            }
        }
    }

    // ══════════════ BLOK BAŞLIĞI ABONELİĞİ ══════════════
    println!();
    println!("{}", "  ════════════════════════════════════════════════════════════════".green());
    println!("  {}  LIVE FEED v9.0 — Listening for new blocks + Pending TX...", "📡".green());
    println!("  {}  Loop: Pending TX → State Sync → TickBitmap → NR → REVM → Execute", "📡".green());
    println!("{}", "  ════════════════════════════════════════════════════════════════".green());
    println!();

    // ══════════════ PENDING TX DİNLEYİCİ (FAZ 4) ══════════════
    // Base L2 sequencer'daki bekleyen swap TX'lerini arka planda dinle
    // ve etkilenen havuzların durumlarını iyimser (optimistic) olarak güncelle.
    // Bu sayede blok onayını beklemeden ~15-20ms erken hareket edilir.
    //
    // v25.0 SINIRLILIK: Bu dinleyici başlangıçtaki havuz listesiyle başlar.
    // Hot-reload ile eklenen yeni havuzlar bu listener tarafından izlenmez.
    // Yeni havuzlar yalnızca blok-bazlı sync ile güncellenir (~2s Base L2).
    // TODO: Havuz listesi değiştiğinde listener'ı yeniden başlat (CancellationToken ile).
    let pool_addresses: Vec<Address> = pools.iter().map(|p| p.address).collect();
    {
        let pools_bg = pools.to_vec();
        let states_bg: Vec<SharedPoolState> = states.iter().map(Arc::clone).collect();
        let pool_addrs_bg = pool_addresses.clone();
        let rpc_url_bg = config.rpc_wss_url.clone();
        let token_bg = cancel_token.clone();

        tokio::spawn(async move {
            tokio::select! {
                _ = token_bg.cancelled() => {
                    eprintln!("  🔌 Pending TX listener graceful shutdown (CancellationToken)");
                }
                result = pending_tx_listener(
                    &rpc_url_bg,
                    &pools_bg,
                    &states_bg,
                    &pool_addrs_bg,
                ) => {
                    match result {
                        Ok(_) => {}
                        Err(e) => {
                            eprintln!(
                                "  ⚠️ Pending TX listener error (block-based flow continues): {}", e
                            );
                        }
                    }
                }
            }
        });
    }

    // ══════════════ SWAP EVENT DİNLEYİCİ (v11.0) ══════════════
    // Havuz swap eventlerini eth_subscribe("logs") ile dinle.
    // Swap eventi sqrtPriceX96, liquidity, tick bilgisini doğrudan içerir —
    // ek RPC çağrısı olmadan state güncellenir (zero-latency).
    //
    // v25.0 SINIRLILIK: Pending TX dinleyicisi ile aynı kısıt — başlangıçtaki
    // havuz listesiyle çalışır, hot-reload ile eklenen yeni havuzları kapsamaz.
    // TODO: Havuz listesi değiştiğinde event listener'ı yeniden başlat.
    {
        let pools_ev = pools.to_vec();
        let states_ev: Vec<SharedPoolState> = states.iter().map(Arc::clone).collect();
        let rpc_url_ev = config.rpc_wss_url.clone();
        let token_ev = cancel_token.clone();

        tokio::spawn(async move {
            tokio::select! {
                _ = token_ev.cancelled() => {
                    eprintln!("  🔌 Swap event listener graceful shutdown (CancellationToken)");
                }
                _result = async {
                    let ws = WsConnect::new(&rpc_url_ev);
                    match ProviderBuilder::default().connect_ws(ws).await {
                        Ok(ws_provider) => {
                            match state_sync::start_swap_event_listener(
                                &ws_provider,
                                &pools_ev,
                                &states_ev,
                            ).await {
                                Ok(_) => {}
                                Err(e) => {
                                    eprintln!(
                                        "  ⚠️ Swap event listener error (block-based flow continues): {}", e
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "  ⚠️ Swap event WS connection error: {}", e
                            );
                        }
                    }
                } => {}
            }
        });
    }

    // ══════════════ KEŞİF MOTORU v25.0 (Otonom Keşif) ══════════════
    // On-Chain Factory Listener + Multi-API Aggregator + Skorlama + GC
    let discovery_config = DiscoveryConfig::from_bot_config(
        &config.rpc_wss_url,
        config.max_pool_fee_bps,
        config.weth_address,
    );
    let discovery_registry = Arc::new(RwLock::new(LivePoolRegistry::new(pools)));
    {
        let engine = DiscoveryEngine::new(discovery_registry.clone(), discovery_config.clone());
        engine.start(cancel_token.clone());
    }

    let sub = provider.subscribe_blocks().await?;
    let mut stream = sub.into_stream();
    let mut stats = ArbitrageStats::new();
    stats.active_transport = active_transport.to_string();
    let mut last_bitmap_block: u64 = block;
    // v14.0: Son REVM simülasyonundan gelen gerçek gas değeri
    // İlk blokta None → check_arbitrage_opportunity 150K fallback kullanır
    // Sonraki bloklarda REVM'den dönen kesin gas ile dinamik maliyet hesaplanır
    let mut last_simulated_gas: Option<u64> = None;

    // v29.0: Hot-Reload arka plan görevi handle'ı
    // Yeni havuz keşfi → bytecode + state sync işlemleri arka planda çalışır,
    // ana ticaret döngüsünü BLOKLAMAZ. Tamamlandığında REVM base_db rebuild edilir.
    let mut hot_reload_task: Option<tokio::task::JoinHandle<()>> = None;

    // ══════════════ ANA DÖNGÜ — BLOK BAZLI + WSS HEARTBEAT ══════════════
    // v10.1: WSS bağlantı sağlığı kontrolü (Heartbeat)
    // 15 saniye içinde yeni blok gelmezse bağlantı kopmuş sayılır
    // ve run_bot() hata döndürerek agresif reconnect tetiklenir.
    // Base L2: ~2s blok süresi → 15s = ~7 blok kaybı toleransı
    loop {
        let block_header = match tokio::time::timeout(
            Duration::from_secs(15),
            stream.next(),
        ).await {
            Ok(Some(header)) => header,
            Ok(None) => {
                // Stream kapandı — reconnect gerekli
                println!(
                    "  {} WSS stream closed — reconnecting...",
                    "⚠️".yellow()
                );
                return Err(eyre::eyre!("WSS stream closed"));
            }
            Err(_) => {
                // 15s timeout — bağlantı muhtemelen koptu
                println!(
                    "  {} WSS heartbeat timeout (no block for 15s) — reconnecting",
                    "💔".red()
                );
                return Err(eyre::eyre!("WSS heartbeat timeout: no block received for 15 seconds"));
            }
        };

        let block_start = Instant::now();
        let block_number = block_header.number;

        // v10.0: Dinamik timestamp ve base_fee — zincir verisinden
        let block_timestamp = block_header.timestamp;
        let block_base_fee = block_header.base_fee_per_gas
            .unwrap_or(0) as u64;

        // ── 1. DURUM SENKRONİZASYONU + L1 FEE + TİCKBİTMAP (PARALEL) ────
        // v27.1: Üç bağımsız I/O işlemi tek tokio::join! ile paralel çalışır.
        // Eski: sync_pools → L1_fee → ... → TickBitmap (sıralı, 600ms+ ekleniyor)
        // Yeni: sync_pools ∥ L1_fee ∥ TickBitmap (paralel, toplam ≈ max(tek RTT))
        //
        // TickBitmap periyodik güncelleme (her N blokta) artık ayrı bir adım değil,
        // state sync ile eşzamanlı çalışır → blok döngüsü 500-600ms kısalır.
        let bitmap_needs_refresh = block_number.saturating_sub(last_bitmap_block)
            >= config.tick_bitmap_max_age_blocks;

        let bitmap_future = async {
            if bitmap_needs_refresh {
                let bm_start = Instant::now();
                let results = sync_all_tick_bitmaps(
                    &provider, pools, &states, block_number, config.tick_bitmap_range,
                ).await;
                Some((results, bm_start.elapsed()))
            } else {
                None
            }
        };

        let (sync_results, l1_data_fee_wei, bitmap_result) = tokio::join!(
            sync_all_pools(&provider, pools, &states, block_number),
            estimate_l1_data_fee(&provider),
            bitmap_future,
        );

        // TickBitmap sonuçlarını işle
        if let Some((bm_results, bm_elapsed)) = bitmap_result {
            let bm_ms = bm_elapsed.as_millis();
            let bm_ok = bm_results.iter().filter(|r| r.is_ok()).count();
            if bm_ok > 0 {
                println!(
                    "     {} TickBitmap updated ({}/{} pools, {}ms) [PARALLEL]",
                    "🗺️".cyan(), bm_ok, pools.len(), bm_ms,
                );
            }
            stats.tick_bitmap_syncs += 1;
            last_bitmap_block = block_number;
        }

        // v27.0: L1 Data Fee teşhis logu — 0 gelmesi OP Stack'te anormal
        let l1_fee_eth = l1_data_fee_wei as f64 / 1e18;
        if l1_data_fee_wei == 0 {
            eprintln!(
                "  ⚠️ [L1 Fee] WARNING: L1 data fee = 0 wei — GasPriceOracle may not be responding!",
            );
        } else {
            eprintln!(
                "  ⛽ [L1 Fee] {} wei ({:.8} ETH)", l1_data_fee_wei, l1_fee_eth,
            );
        }

        let sync_ms = block_start.elapsed().as_millis();

        // Gecikme ölçümü
        stats.update_latency(sync_ms as f64);

        // v15.0: Gecikme spike tespiti ve uyarısı
        if (sync_ms as f64) > config.latency_spike_threshold_ms {
            stats.latency_spikes += 1;
            eprintln!(
                "  ⚡ [Block #{}] Latency SPIKE: {}ms (threshold: {:.0}ms) — #{} spike", block_number, sync_ms,
                config.latency_spike_threshold_ms,
                stats.latency_spikes,
            );
        }

        let all_synced = sync_results.iter().all(|r| r.is_ok());

        // Hata raporlama
        for (i, result) in sync_results.iter().enumerate() {
            if let Err(e) = result {
                println!(
                    "  {} [Block #{}] {} sync error: {}",
                    "⚠️".yellow(), block_number, pools[i].name, e
                );
            }
        }

        stats.total_blocks_processed += 1;

        // ── 1.4. KEŞİF MOTORU: HOT-RELOAD + GC + SKORLAMA ─────

        // v29.0: Önceki arka plan hot-reload tamamlandı mı kontrol et
        // Tamamlandıysa REVM base_db'yi yeniden oluştur (yeni havuz bytecode'ları dahil)
        if let Some(ref handle) = hot_reload_task {
            if handle.is_finished() {
                let handle = hot_reload_task.take().unwrap();
                if let Ok(()) = handle.await {
                    sim_engine.cache_bytecodes(pools, &states);
                    let reload_caller = executor_address.unwrap_or_default();
                    let reload_contract = config.contract_address.unwrap_or_default();
                    sim_engine.initialize_base_db(pools, &states, reload_caller, reload_contract);
                    eprintln!(
                        "  🔧 [Hot-Reload BG] REVM base_db rebuilt ({} pools)", pools.len(),
                    );
                }
            }
        }

        // [Adım 3] Bekleyen havuzları canlı sisteme enjekte et
        let hot_reload_count = discovery_engine::apply_pending_updates(
            &discovery_registry, pools, &mut states, pair_combos,
        );
        if hot_reload_count > 0 {
            // v29.0: Bytecode + State sync + TickBitmap sync ARKA PLANDA çalışır
            // Ana ticaret döngüsü BLOKLANMAZ — keşif sırasında fiyat okumaya devam eder.
            // Yeni havuzlar sync tamamlanana kadar STALE kalır → arb pipeline atlar.
            let new_start = pools.len() - hot_reload_count;
            let bg_provider = provider.clone();
            let bg_pools: Vec<PoolConfig> = pools[new_start..].to_vec();
            let bg_states: Vec<SharedPoolState> = states[new_start..].to_vec();
            let bg_bitmap_range = config.tick_bitmap_range;
            let bg_block = block_number;

            hot_reload_task = Some(tokio::spawn(async move {
                // Adım 1: Bytecode al (paralel)
                let bytecode_futs: Vec<_> = bg_pools.iter().enumerate()
                    .map(|(i, pool)| {
                        let provider = &bg_provider;
                        let addr = pool.address;
                        async move { (i, provider.get_code_at(addr).await) }
                    })
                    .collect();
                let bytecode_results = join_all(bytecode_futs).await;
                for (i, result) in bytecode_results {
                    if let Ok(code) = result {
                        if !code.is_empty() {
                            bg_states[i].write().bytecode = Some(code.to_vec());
                        }
                    }
                }

                // Adım 2: State sync + TickBitmap sync (paralel)
                let sync_futs: Vec<_> = bg_pools.iter().enumerate()
                    .map(|(i, pool_cfg)| {
                        let provider = &bg_provider;
                        let pool_state = &bg_states[i];
                        let bitmap_range = bg_bitmap_range;
                        async move {
                            if let Err(e) = crate::state_sync::sync_pool_state(
                                provider, pool_cfg, pool_state, bg_block,
                            ).await {
                                eprintln!("  ⚠️ [Hot-Reload BG] {} state sync failed: {}", pool_cfg.name, e);
                            }
                            if let Err(e) = crate::state_sync::sync_tick_bitmap(
                                provider, pool_cfg, pool_state, bg_block, bitmap_range,
                            ).await {
                                eprintln!("  ⚠️ [Hot-Reload BG] {} bitmap sync failed: {}", pool_cfg.name, e);
                            }
                        }
                    })
                    .collect();
                join_all(sync_futs).await;
                eprintln!(
                    "  ✅ [Hot-Reload BG] {} new pools synced in background", bg_pools.len(),
                );
            }));

            // v25.0: Yeni havuzları on-chain pool whitelist'e ekle
            // executorBatchAddPools() ile executor key'i kullanarak whitelist güncellenir.
            // Kontrat v25.0: executor yalnızca EKLEME yapabilir (güvenlik korunur).
            if config.execution_enabled() {
                let new_addrs: Vec<Address> = pools[new_start..].iter().map(|p| p.address).collect();
                if !new_addrs.is_empty() {
                    let calldata = crate::executor::encode_whitelist_calldata(&new_addrs);
                    if let (Some(ref pk), Some(contract_addr)) = (&config.private_key, config.contract_address) {
                        let pk_clone = pk.clone();
                        let mev_exec_clone = Arc::clone(&mev_executor);
                        let nonce = nonce_manager.get_and_increment();
                        let nm_clone = Arc::clone(&nonce_manager);
                        let base_fee = block_base_fee;
                        let addr_count = new_addrs.len();
                        tokio::spawn(async move {
                            match whitelist_pools_on_chain(
                                mev_exec_clone, pk_clone, contract_addr,
                                calldata, nonce, nm_clone, base_fee,
                            ).await {
                                Ok(_) => eprintln!(
                                    "  ✅ [Whitelist] {} pools added to on-chain whitelist", addr_count,
                                ),
                                Err(e) => eprintln!(
                                    "  ⚠️ [Whitelist] On-chain whitelist error: {} — admin must add manually", e,
                                ),
                            }
                        });
                    }
                }
            }
        }

        // [Adım 4] Çöp Toplayıcı — soğuk havuzları temizle
        let _gc_deactivated = discovery_engine::run_garbage_collector(
            &discovery_registry, pools, block_number, &discovery_config,
        );

        // [Adım 5] Skor güncelleme (her ~10 dakikada bir)
        discovery_engine::update_scores(
            &discovery_registry, pools, block_number, &discovery_config,
        );

        // Spread gözlemlerini kaydet (skorlama için)
        for combo in pair_combos.iter() {
            discovery_engine::record_spread_observation(
                &discovery_registry, combo.pool_a_idx, combo.pool_b_idx, &states,
            );
        }

        // ── 1.5. TİCKBİTMAP — Artık §1'de paralel çalışıyor (v27.1) ──

        // ── 2. BLOK + SPREAD BİLGİSİ ───────────────────────
        print_block_update(block_number, pools, &states, sync_ms);
        for combo in pair_combos.iter() {
            let pp = [pools[combo.pool_a_idx].clone(), pools[combo.pool_b_idx].clone()];
            let ps = [states[combo.pool_a_idx].clone(), states[combo.pool_b_idx].clone()];
            print_spread_info(&pp, &ps);
        }

        // ── 2.5. SPREAD İSTATİSTİK GÜNCELLEMESİ (Her blokta) ────────
        // v15.0 FIX: max_spread ve total_opportunities güncelleme
        // fırsat değerlendirmesinden BAĞIMSIZ olarak her blokta çalışır.
        // Önceki sürümde bu istatistikler sadece evaluate_and_execute()
        // içinde güncelleniyordu — NR kârsız bulursa hiç çağrılmıyordu.
        for combo in pair_combos.iter() {
            let sa = states[combo.pool_a_idx].read();
            let sb = states[combo.pool_b_idx].read();
            if sa.is_active() && sb.is_active() {
                let spread = (sa.eth_price_usd - sb.eth_price_usd).abs();
                let min_p = sa.eth_price_usd.min(sb.eth_price_usd);
                if min_p > 0.0 {
                    let spread_pct = (spread / min_p) * 100.0;
                    if spread_pct > stats.max_spread_pct {
                        stats.max_spread_pct = spread_pct;
                    }
                    if spread_pct > 0.001 {
                        stats.total_opportunities += 1;
                    }
                }
            }
        }

        // ── 3. ARBİTRAJ FIRSATI KONTROLÜ ────────────────────
        // v28.0: Pipeline Bütçesi — toplam blok işleme süresi Base L2 blok
        // süresinin (2s) %75'ini aşıyorsa, gönderilecek TX hedef bloğu
        // kaçıracağı için işlem atlanır. Eski/gecikmeli veriyle yapılan
        // simülasyonlar frontrun ve sandwich saldırılarına açıktır.
        let pipeline_elapsed_ms = block_start.elapsed().as_millis();
        const PIPELINE_BUDGET_MS: u128 = 1500; // Base L2 ~2s blok, %75 bütçe

        if all_synced && pipeline_elapsed_ms <= PIPELINE_BUDGET_MS {
            for (combo_idx, combo) in pair_combos.iter().enumerate() {
                // v25.0: GC tarafından deaktive edilmiş havuzları atla
                {
                    let reg = discovery_registry.read();
                    if !reg.is_active(combo.pool_a_idx) || !reg.is_active(combo.pool_b_idx) {
                        continue;
                    }
                }

                // ── v11.0: Cool-down Blacklist kontrolü ──────────────
                // Bu çift blacklist'te mi? (current_block + 100 blok engeli)
                if let Some(&until_block) = pair_cooldown.get(&combo_idx) {
                    if block_number < until_block {
                        // Hâlâ cool-down'da — bu çifti atla, diğerlerine devam et
                        if block_number % 25 == 0 {
                            // Her 25 blokta bir hatırlatma logu
                            eprintln!(
                                "     \u{26d4} [Blacklist] {} \u{2192} blocked until block #{} (remaining: {} blocks)", combo.pair_name, until_block,
                                until_block.saturating_sub(block_number),
                            );
                        }
                        continue;
                    } else {
                        // Cool-down süresi doldu — çifti yeniden aktif et
                        pair_cooldown.remove(&combo_idx);
                        pair_failures.remove(&combo_idx);
                        eprintln!(
                            "     \u{2705} [Blacklist] {} cool-down expired — reactivated", combo.pair_name,
                        );
                    }
                }

                let pp = [pools[combo.pool_a_idx].clone(), pools[combo.pool_b_idx].clone()];
                let ps = [states[combo.pool_a_idx].clone(), states[combo.pool_b_idx].clone()];
                if let Some(opportunity) = check_arbitrage_opportunity(&pp, &ps, config, block_base_fee, last_simulated_gas, l1_data_fee_wei) {
                    // ── 4. DEĞERLENDİR + SİMÜLE + YÜRÜT ────────────────
                    if let Some(gas) = evaluate_and_execute(
                        &provider,
                        config,
                        &pp,
                        &ps,
                        &opportunity,
                        &sim_engine,
                        &mut stats,
                        &nonce_manager,
                        block_timestamp,
                        block_base_fee,
                        sync_ms as f64,
                        l1_data_fee_wei,
                        &mev_executor,
                    ).await {
                        last_simulated_gas = Some(gas);
                        // Başarılı simülasyon — bu çift için hata sayacını sıfırla
                        pair_failures.remove(&combo_idx);
                    } else {
                        // evaluate_and_execute None döndü → simülasyon başarısız
                        // Per-pair ardışık hata sayacını artır
                        let failures = pair_failures.entry(combo_idx).or_insert(0);
                        *failures += 1;

                        if *failures >= config.circuit_breaker_threshold {
                            // Bu çifti 100 blok boyunca blacklist'e al
                            let cooldown_until = block_number + 100;
                            pair_cooldown.insert(combo_idx, cooldown_until);
                            eprintln!(
                                "\n  \u{1f6d1} CIRCUIT BREAKER: {} {} consecutive failures — blacklisted until block #{} (~{}s)",
                                combo.pair_name,
                                failures,
                                cooldown_until,
                                100 * 2, // Base L2 ~2s blok süresi
                            );
                            // Global stats'ı da güncelle
                            stats.consecutive_failures = 0;
                        }
                    }
                }
            }
        } else if pipeline_elapsed_ms > PIPELINE_BUDGET_MS {
            // v28.0: Pipeline bütçesi aşıldı — bu bloğu atla
            eprintln!(
                "  \u{26a0}\u{fe0f} [Pipeline] Block #{} processing time {}ms > budget {}ms — opportunity scan skipped (MEV protection)",
                block_number, pipeline_elapsed_ms, PIPELINE_BUDGET_MS,
            );
        }

        // ── 4. MULTI-HOP ROTA TARAMASI (v25.0: Simülasyon + Yürütme) ─────
        //    LiquidityGraph'ı mevcut havuz verileriyle oluştur,
        //    3+ hop rotalarını tara ve kârlı olanları yürüt.
        if all_synced && block_number % 3 == 0 {
            let graph = route_engine::LiquidityGraph::build(pools, &states, config.weth_address);
            let routes = graph.find_routes(4, 200);

            if !routes.is_empty() {
                let two_hop_count = graph.two_hop_routes(&routes).len();
                let multi_hop_routes = graph.multi_hop_routes(&routes);
                let multi_hop_count = multi_hop_routes.len();

                eprintln!(
                    "     {} [Graph] {} nodes, {} edges | {} routes (2-hop: {}, 3+hop: {})",
                    "🔀".cyan(),
                    graph.node_count(),
                    graph.edge_count(),
                    routes.len(),
                    two_hop_count,
                    multi_hop_count,
                );

                let multi_hop_opps = strategy::check_multi_hop_opportunities(
                    &routes,
                    pools,
                    &states,
                    config,
                    block_base_fee,
                    l1_data_fee_wei,
                );

                if let Some(best) = multi_hop_opps.first() {
                    // Exact U256 profit doğrulaması
                    let amount_wei = crate::math::exact::f64_to_u256_wei(best.optimal_amount_weth);
                    let pool_states_ex: Vec<crate::types::PoolState> = best.pool_indices.iter()
                        .map(|&i| states[i].read().clone()).collect();
                    let pool_configs_ex: Vec<&crate::types::PoolConfig> = best.pool_indices.iter()
                        .map(|&i| &pools[i]).collect();
                    let state_refs_ex: Vec<&crate::types::PoolState> = pool_states_ex.iter().collect();
                    let exact_profit = crate::math::compute_exact_profit_multi_hop(
                        &state_refs_ex, &pool_configs_ex, &best.directions, amount_wei,
                    );

                    // Calldata boyutu hesapla
                    let pool_addrs: Vec<alloy::primitives::Address> = best.pool_indices.iter()
                        .map(|&i| pools[i].address).collect();
                    let dirs_u8: Vec<u8> = best.directions.iter()
                        .map(|&d| if d { 0u8 } else { 1u8 }).collect();
                    let calldata = crate::simulator::encode_multi_hop_calldata(
                        &pool_addrs, &dirs_u8, amount_wei, 0, 0,
                    );

                    eprintln!(
                        "     {} [Multi-Hop] #{} {} | {:.4} WETH → {:.6} WETH profit | {} | exact={} wei | {}B calldata | {}-hop NR({}/{})",
                        "🔀".cyan(),
                        best.route_idx,
                        best.label,
                        best.optimal_amount_weth,
                        best.expected_profit_weth,
                        if best.nr_converged { "✓" } else { "~" },
                        exact_profit,
                        calldata.len(),
                        best.hop_count,
                        best.nr_iterations,
                        if best.nr_converged { "converged" } else { "scan" },
                    );

                    // v25.0: Multi-hop fırsatı değerlendir ve yürüt
                    if let Some(gas) = strategy::evaluate_and_execute_multi_hop(
                        &provider,
                        config,
                        pools,
                        &states,
                        best,
                        &sim_engine,
                        &mut stats,
                        &nonce_manager,
                        block_timestamp,
                        block_base_fee,
                        sync_ms as f64,
                        l1_data_fee_wei,
                        &mev_executor,
                    ).await {
                        last_simulated_gas = Some(gas);
                    }
                }
            }
        }

        // ── 5. PERİYODİK İSTATİSTİK ────────────────────────
        if stats.total_blocks_processed.is_multiple_of(config.stats_interval)
            && stats.total_blocks_processed > 0
        {
            print_stats_summary(&stats, &states, pools, pair_combos);
            // Keşif motoru istatistikleri
            discovery_engine::print_discovery_stats(&discovery_registry, pools);
        }

        // ── 6. PERİYODİK NONCE SENKRONİZASYONU (v10.0) ──────
        // Her 50 blokta bir zincirdeki gerçek nonce ile lokal nonce'u karşılaştır.
        // Uyumsuzluk varsa zincir değeri ile düzelt (TX kayıpları veya dış müdahale).
        if stats.total_blocks_processed.is_multiple_of(50)
            && stats.total_blocks_processed > 0
        {
            if let Some(addr) = executor_address {
                match provider.get_transaction_count(addr).await {
                    Ok(onchain_nonce) => {
                        let local_nonce = nonce_manager.current();
                        if local_nonce != onchain_nonce {
                            println!(
                                "  {} Nonce mismatch detected: local={} chain={} → correcting",
                                "🔄".yellow(), local_nonce, onchain_nonce
                            );
                            nonce_manager.force_set(onchain_nonce);
                        }
                    }
                    Err(e) => {
                        println!("  {} Nonce sync failed: {}", "⚠️".yellow(), e);
                    }
                }
            }
        }
    } // heartbeat loop sonu — loop sadece return Err() ile çıkar
}

// ─────────────────────────────────────────────────────────────────────────────
// PENDING TX DİNLEYİCİ (FAZ 4) — Optimistic State Update
// ─────────────────────────────────────────────────────────────────────────────
//
// Base L2 sequencer'daki bekleyen işlemleri WebSocket üzerinden dinler.
// İzlenen havuzlara (UniV3 / Slipstream) yönelik swap TX'leri tespit
// edildiğinde, havuzun durumu anlık olarak RPC'den tekrar okunur.
//
// Bu "iyimser güncelleme" sayesinde bot, blok onayını beklemeden
// ~15-20ms erken hareket edebilir.
//
// NOT: Base L2'de mempool sınırlıdır. Bu dinleyici "best effort" çalışır.
// Pending TX bulunamasa bile mevcut blok bazlı akış aynen devam eder.
// ─────────────────────────────────────────────────────────────────────────────

async fn pending_tx_listener(
    rpc_url: &str,
    pools: &[PoolConfig],
    states: &[SharedPoolState],
    pool_addresses: &[Address],
) -> Result<()> {
    use alloy::providers::WsConnect;

    let ws = WsConnect::new(rpc_url);
    let provider = ProviderBuilder::default().connect_ws(ws).await
        .map_err(|e| eyre::eyre!("Pending TX provider connection error: {}", e))?;

    println!("  {} Pending TX listener started (optimistic mode)", "🔮".cyan());

    // Pending TX stream — full TX nesneleri ile
    let sub: alloy::pubsub::Subscription<alloy::rpc::types::Transaction> = provider.subscribe_full_pending_transactions().await
        .map_err(|e| eyre::eyre!("Pending TX subscription error: {}", e))?;
    let mut stream = sub.into_stream();

    while let Some(tx) = stream.next().await {
        // TX'in hedef adresi izlenen havuzlardan biri mi?
        use alloy::consensus::Transaction as TxTrait;
        let tx_kind = TxTrait::kind(&*tx.inner);
        let tx_to = tx_kind.to().copied();
        let tx_input = TxTrait::input(&*tx.inner);

        if let Some(pool_idx) = state_sync::check_pending_tx_relevance(
            tx_to,
            tx_input,
            pool_addresses,
        ) {
            // Etkilenen havuzun durumunu anlık oku (optimistic refresh)
            let current_block = states[0].read().last_block;
            match state_sync::optimistic_refresh_pool(
                &provider,
                &pools[pool_idx],
                &states[pool_idx],
                current_block,
            ).await {
                Ok(true) => {
                    // Fiyat değişti — havuz güncellendi
                    let state = states[pool_idx].read();
                    println!(
                        "     {} [Pending TX] {} optimistic update: {:.6} Q",
                        "🔮".magenta(),
                        pools[pool_idx].name,
                        state.eth_price_usd,
                    );
                }
                Ok(false) => {} // Fiyat değişmedi, sessiz geç
                Err(e) => {
                    // Hata — sessiz devam et, blok bazlı akış zaten çalışıyor
                    eprintln!(
                        "     ⚠️ [Pending TX] {} refresh error: {}", pools[pool_idx].name, e
                    );
                }
            }
        }
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// v25.0: On-Chain Pool Whitelist Güncelleme
// ─────────────────────────────────────────────────────────────────────────────
//
// Keşif motoru yeni havuz bulduğunda, kontratın executorBatchAddPools()
// fonksiyonu çağrılarak havuz on-chain whiteliste eklenir.
// Bu işlem fire-and-forget tokio::spawn ile çalışır — ana döngüyü bloklamaz.
// ─────────────────────────────────────────────────────────────────────────────

async fn whitelist_pools_on_chain(
    mev_executor: Arc<executor::MevExecutor>,
    private_key: String,
    contract_address: Address,
    calldata: Vec<u8>,
    nonce: u64,
    nonce_manager: Arc<NonceManager>,
    block_base_fee: u64,
) -> Result<()> {
    use alloy::signers::local::PrivateKeySigner;
    use alloy::network::EthereumWallet;

    let signer: PrivateKeySigner = private_key.parse()
        .map_err(|e| eyre::eyre!("Whitelist TX: key parse error: {}", e))?;
    let wallet = EthereumWallet::from(signer.clone());

    // Basit TX gönderimi — whitelist işlemi düşük öncelikli
    let ws = alloy::providers::WsConnect::new(mev_executor.standard_rpc_url());
    let provider = ProviderBuilder::new()
        .wallet(wallet)
        .connect_ws(ws).await
        .map_err(|e| eyre::eyre!("Whitelist TX: provider error: {}", e))?;

    let max_fee = block_base_fee as u128 + 1_000_000_000; // base_fee + 1 Gwei tip
    let tx = alloy::rpc::types::TransactionRequest::default()
        .to(contract_address)
        .input(calldata.into())
        .nonce(nonce)
        .gas_limit(100_000) // Whitelist işlemi ~30K gas
        .max_fee_per_gas(max_fee)
        .max_priority_fee_per_gas(1_000_000_000); // 1 Gwei — düşük öncelik yeterli

    match provider.send_transaction(tx).await {
        Ok(pending) => {
            eprintln!("  📤 [Whitelist] TX sent: {:?}", pending.tx_hash());
            // Fire-and-forget — receipt beklenmez
            Ok(())
        }
        Err(e) => {
            // Nonce'u geri al (TX gönderilemedi)
            nonce_manager.force_set(nonce);
            Err(eyre::eyre!("Whitelist TX failed to send: {}", e))
        }
    }
}
