// ============================================================================
//  ARBITRAJ BOTU v6.0 — "Kuantum Beyin II"
//  Base Network Çapraz-DEX Arbitraj Sistemi
//
//  v6.0 Devrim Niteliğinde Yenilikler:
//  ✓ Off-Chain TickBitmap Derinlik Simülasyonu (Gerçek Multi-Tick)
//  ✓ Multi-Transport Bağlantı (IPC > WSS > HTTP — Sub-1ms Hedefi)
//  ✓ Base L2 Sequencer Optimizasyonu (FIFO-Aware)
//  ✓ Gecikme Ölçümü ve İstatistikleri
//
//  v5.0 (korunuyor):
//  ✓ Yerel Durum Senkronizasyonu (Event/Mempool yerine State Sync)
//  ✓ REVM ile Yerel Simülasyon (eth_call yerine — 0 gecikme)
//  ✓ Newton-Raphson Optimal Hacim (Sabit TRADE_SIZE yerine — Dinamik)
//  ✓ Uniswap V3 + Aerodrome CL çapraz-DEX desteği
//  ✓ Modüler mimari (types, math, state_sync, simulator, strategy)
// ============================================================================

mod types;
mod math;
mod state_sync;
mod simulator;
mod strategy;
mod key_manager;

use types::*;
use state_sync::*;
use simulator::SimulationEngine;
use strategy::*;

use alloy::primitives::Address;
use alloy::providers::{Provider, ProviderBuilder, WsConnect};
use futures_util::StreamExt;
use eyre::Result;
use chrono::Local;
use colored::*;
use std::sync::Arc;
use std::time::{Duration, Instant};
use parking_lot::RwLock;

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
        "║       ARBITRAJ BOTU v9.0 — Kuantum Beyin III                   ║"
            .cyan().bold()
    );
    println!(
        "{}",
        "║    Base Network Çapraz-DEX Arbitraj Sistemi                     ║"
            .cyan().bold()
    );
    println!(
        "{}",
        "╠══════════════════════════════════════════════════════════════════╣"
            .cyan().bold()
    );
    println!(
        "{}",
        "║  [v9] Executor/Admin Rol Ayrımı + Deadline Block               ║"
            .cyan()
    );
    println!(
        "{}",
        "║  [v9] Şifreli Key Management (AES-256-GCM + PBKDF2)            ║"
            .cyan()
    );
    println!(
        "{}",
        "║  [v9] Dinamik Bribe/Priority Fee + 134-Byte Calldata           ║"
            .cyan()
    );
    println!(
        "{}",
        "║  [v6] TickBitmap + Multi-Tick Derinlik + REVM Simülasyon        ║"
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
    println!("  {} Motor          : {}", "▸".cyan(), "Rust + Alloy + REVM (Sıfır Gecikme)".white());
    println!("  {} Ağ             : {}", "▸".cyan(), format!("Base Network (Chain ID: {})", config.chain_id).white());
    println!("  {} Transport      : {}", "▸".cyan(), format!("{:?} (Öncelik: IPC > WSS > HTTP)", config.transport_mode).white());
    println!("  {} Strateji       : {}", "▸".cyan(), "Çapraz-DEX Spread Arbitrajı (Uniswap V3 + Aerodrome)".white());
    println!("  {} Derinlik       : {}", "▸".cyan(), format!("TickBitmap (±{} tick aralığı, max {}blk yaş)", config.tick_bitmap_range, config.tick_bitmap_max_age_blocks).white());
    println!("  {} Calldata       : {}", "▸".cyan(), format!("134 byte kompakt (deadline: +{} blok)", config.deadline_blocks).white());
    println!("  {} Bribe          : {}", "▸".cyan(), format!("Dinamik %{:.0} kâr → priority fee", config.bribe_pct * 100.0).white());
    println!("  {} Key Yönetimi   : {}", "▸".cyan(), if config.key_manager_active { "Şifreli Keystore (AES-256-GCM)".green().to_string() } else if config.private_key.is_some() { "Env Var (GÜVENSİZ)".yellow().to_string() } else { "Yok".red().to_string() });
    println!("  {} Flash Loan     : {}", "▸".cyan(), format!("Aave V3 (%{:.2} Komisyon)", config.flash_loan_fee_bps / 100.0).white());
    println!("  {} Maks İşlem     : {}", "▸".cyan(), format!("{:.1} WETH", config.max_trade_size_weth).white());
    println!("  {} Min. Net Kâr   : {}", "▸".cyan(), format!("{:.2}$", config.min_net_profit_usd).white());
    println!(
        "  {} Başlangıç      : {}",
        "▸".cyan(),
        Local::now().format("%Y-%m-%d %H:%M:%S").to_string().yellow()
    );
    println!(
        "  {} Mod            : {}",
        "▸".cyan(),
        if config.execution_enabled() {
            "CANLI (Kontrat Tetikleme Aktif)".green().bold().to_string()
        } else if config.shadow_mode() {
            "GÖLGE MODU (Kuru Sıkı — shadow_logs.json'a kayıt)".yellow().bold().to_string()
        } else {
            "GÖZLEM (Sadece İzleme)".yellow().bold().to_string()
        }
    );
    println!();
}

fn print_pool_header(pools: &[PoolConfig]) {
    println!("{}", "  ┌──────────────────────────────────────────────────────────────┐".dimmed());
    println!("  {} {}", "│".dimmed(), "Gözetlenen Havuzlar:".white().bold());
    for (i, p) in pools.iter().enumerate() {
        let icon = if i == 0 { "🔵" } else { "🟣" };
        println!(
            "  {}   {} {} ({} — Ücret: %{:.2})",
            "│".dimmed(),
            icon,
            p.name,
            p.dex,
            p.fee_bps as f64 / 100.0
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
                "{}={:.2}$",
                short_name,
                state.eth_price_usd,
            ));
        }
    }

    println!(
        "  {} [{}] Blok #{} | {} | Senk: {}ms",
        "🧱".blue(),
        timestamp().dimmed(),
        format!("{}", block_number).white().bold(),
        pool_info,
        sync_ms,
    );
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
                "     {} Spread: {:.4}% ({:.4}$) | {} AL→SAT",
                "📊".yellow(), spread_pct, spread, direction,
            );
        } else {
            println!(
                "     {} Spread: {:.4}% ({:.4}$) | {}",
                "📊", spread_pct, spread, direction,
            );
        }
    }
}

fn print_stats_summary(stats: &ArbitrageStats, states: &[SharedPoolState]) {
    println!();
    println!("{}", "  ┌───── OTURUM İSTATİSTİKLERİ (v9.0) ──────────────────────────┐".yellow());
    println!("  {}  Çalışma Süresi       : {}", "│".yellow(), stats.uptime_str().white().bold());
    println!("  {}  İşlenen Blok         : {}", "│".yellow(), format!("{}", stats.total_blocks_processed).white());
    println!("  {}  Tespit Edilen Fırsat  : {}", "│".yellow(), format!("{}", stats.total_opportunities).white());
    println!(
        "  {}  Net Kârlı Fırsat     : {}",
        "│".yellow(),
        if stats.profitable_opportunities > 0 {
            format!("{}", stats.profitable_opportunities).green().bold().to_string()
        } else {
            format!("{}", stats.profitable_opportunities).dimmed().to_string()
        }
    );
    println!("  {}  Başarısız Simülasyon  : {}", "│".yellow(), stats.failed_simulations);
    println!(
        "  {}  Yürütülen İşlem      : {}",
        "│".yellow(),
        if stats.executed_trades > 0 {
            format!("{}", stats.executed_trades).green().bold().to_string()
        } else {
            format!("{}", stats.executed_trades).dimmed().to_string()
        }
    );
    println!("  {}  Maks. Spread          : {:.4}%", "│".yellow(), stats.max_spread_pct);
    println!("  {}  Maks. Kâr (tek)       : {:.4}$", "│".yellow(), stats.max_profit_usd);
    println!("  {}  Toplam Pot. Kâr       : {:.4}$", "│".yellow(), stats.total_potential_profit);

    // v6.0: Gecikme istatistikleri
    println!("  {} ─── Gecikme (State Sync) ──────────────", "│".yellow());
    println!("  {}  Ort. Gecikme          : {:.1}ms", "│".yellow(), stats.avg_block_latency_ms);
    println!("  {}  Min. Gecikme          : {:.1}ms", "│".yellow(), stats.min_block_latency_ms);
    println!("  {}  TickBitmap Sync       : {} kez", "│".yellow(), stats.tick_bitmap_syncs);

    for (i, state_lock) in states.iter().enumerate() {
        let state = state_lock.read();
        if state.is_active() {
            let bitmap_info = if let Some(ref bm) = state.tick_bitmap {
                format!(" | Bitmap: {} tick", bm.ticks.len())
            } else {
                " | Bitmap: YOK".to_string()
            };
            println!(
                "  {}  Havuz {} Fiyat       : {:.2}$ (tick: {}){}",
                "│".yellow(), i + 1, state.eth_price_usd, state.tick, bitmap_info,
            );
        }
    }

    println!("{}", "  └──────────────────────────────────────────────────────────────┘".yellow());
    println!();
}

// ─────────────────────────────────────────────────────────────────────────────
// ANA GİRİŞ NOKTASI — Yeniden Bağlanma Döngüsü
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    // .env dosyasını yükle
    dotenvy::dotenv().ok();

    // ═══ CLI: --encrypt-key argümanı ile keystore oluşturma ═══
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--encrypt-key") {
        return key_manager::KeyManager::cli_encrypt_key();
    }

    // Yapılandırmayı oku
    let mut config = BotConfig::from_env()?;

    // Havuz yapılandırmalarını oku
    let pools = load_pool_configs_from_env()?;

    // ═══ v10.1: TOKEN WHITELIST DOĞRULAMA ═══
    // Startup sırasında yapılandırılan token adreslerini beyaz listeye karşı doğrula
    // v13.0: Aktif edildi — sahte token adresi ile başlatmayı engeller
    crate::types::validate_token_whitelist(&config.weth_address, &config.usdc_address)?;
    println!(
        "  {} Token Whitelist: WETH ve USDC adresleri doğrulandı",
        "✅".green()
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
            "  {} Key Yönetimi: {}",
            "🔐".green(),
            key_manager.source()
        );
    } else {
        println!(
            "  {} Key Yönetimi: Anahtar yüklenmedi (gözlem modu)",
            "ℹ️".blue()
        );
    }

    // Banner göster
    print_banner(&config);
    print_pool_header(&pools);

    // Yeniden bağlanma döngüsü
    let mut retry_count: u32 = 0;

    loop {
        if retry_count > 0 {
            println!(
                "  {} Yeniden bağlanma denemesi #{}",
                "🔄".yellow(), retry_count
            );
        }

        match run_bot(&config, &pools).await {
            Ok(_) => {
                println!(
                    "\n  {} Bağlantı kesildi. Yeniden bağlanılıyor...",
                    "⚠️".yellow()
                );
            }
            Err(e) => {
                println!(
                    "\n  {} Hata: {:#}",
                    "❌".red(), e
                );
            }
        }

        retry_count += 1;

        if config.max_retries > 0 && retry_count >= config.max_retries {
            println!(
                "  {} Maksimum deneme ({}) aşıldı. Bot kapatılıyor.",
                "🛑".red(), config.max_retries
            );
            return Err(eyre::eyre!("Maksimum yeniden bağlanma denemesi aşıldı"));
        }

        // v13.0: Akıllı reconnect — ilk 3 deneme hızlı, sonra exponential backoff
        // İlk kopmalarda hızlı geri dönüş, uzun süren kesintilerde rate-limit koruması.
        let delay_ms = if retry_count <= 3 {
            100u64 // İlk 3 deneme: 100ms (agresif)
        } else {
            // Exponential backoff: 200ms → 400ms → 800ms → ... → max 10s
            let exp_delay = 100u64 * (1u64 << (retry_count - 3).min(6));
            exp_delay.min(10_000) // Üst sınır: 10 saniye
        };
        println!(
            "  {} {}ms sonra yeniden bağlanılıyor... (deneme #{})",
            "⚡".yellow(), delay_ms, retry_count
        );
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BOT MOTORU — Blok Dinle → State Sync → Fırsat Tara → Simüle → Yürüt
// ─────────────────────────────────────────────────────────────────────────────

async fn run_bot(config: &BotConfig, pools: &[PoolConfig]) -> Result<()> {
    // ══════════════ MULTI-TRANSPORT BAĞLANTI ══════════════
    // Öncelik sırası: IPC > WSS > HTTP
    // Base L2 Sequencer'a en düşük gecikme için IPC tercih edilir
    // Not: Blok aboneliği (subscribe_blocks) sadece WS/IPC üzerinden çalışır
    println!("  {} Transport bağlantısı kuruluyor ({:?} mod)...", "⏳".yellow(), config.transport_mode);
    let connect_start = Instant::now();

    let (provider, active_transport) = match config.transport_mode {
        TransportMode::Ipc => {
            // IPC: Yerel düğüm ile en düşük gecikmeyi sağlar
            // alloy "full" feature ile IPC desteği gelecek — şimdilik WSS fallback
            let ipc_path = config.rpc_ipc_path.as_deref().unwrap_or("");
            if !ipc_path.is_empty() {
                println!("  {} IPC ({}) henüz desteklenmiyor, WSS'ye düşülüyor...", "⚠️".yellow(), ipc_path);
            }
            println!("  {} WSS bağlantısı kuruluyor (IPC fallback)...", "🌐".cyan());
            let ws = WsConnect::new(&config.rpc_wss_url);
            let p = ProviderBuilder::new().on_ws(ws).await?;
            let ms = connect_start.elapsed().as_millis();
            println!("  {} WSS bağlantı kuruldu! ({}ms)", "✅".green(), ms);
            (p, "WSS (IPC fallback)")
        }
        TransportMode::Http => {
            // HTTP: Polling gerektirir, subscribe_blocks çalışmaz
            // WSS'ye düş çünkü blok aboneliği lazım
            println!("  {} HTTP modu seçildi ama blok aboneliği WSS gerektirir. WSS kullanılacak.", "⚠️".yellow());
            let ws = WsConnect::new(&config.rpc_wss_url);
            let p = ProviderBuilder::new().on_ws(ws).await?;
            let ms = connect_start.elapsed().as_millis();
            println!("  {} WSS bağlantı kuruldu! ({}ms)", "✅".green(), ms);
            (p, "WSS (HTTP yerine)")
        }
        TransportMode::Ws => {
            println!("  {} WSS bağlantısı kuruluyor...", "🌐".cyan());
            let ws = WsConnect::new(&config.rpc_wss_url);
            let p = ProviderBuilder::new().on_ws(ws).await?;
            let ms = connect_start.elapsed().as_millis();
            println!("  {} WSS bağlantı kuruldu! ({}ms)", "✅".green(), ms);
            (p, "WSS")
        }
        TransportMode::Auto => {
            // Auto: IPC desteği geldiğinde önce IPC denenecek, şimdi WSS
            println!("  {} [Auto] WSS bağlantısı kuruluyor...", "🌐".cyan());
            let ws = WsConnect::new(&config.rpc_wss_url);
            let p = ProviderBuilder::new().on_ws(ws).await?;
            let ms = connect_start.elapsed().as_millis();
            println!("  {} [Auto] WSS bağlantı kuruldu! ({}ms)", "✅".green(), ms);
            (p, "WSS (Auto)")
        }
    };

    let total_connect_ms = connect_start.elapsed().as_millis();

    // Son blok
    let block = provider.get_block_number().await?;
    println!(
        "  {} Güncel blok: #{} | Transport: {} | Bağlantı: {}ms",
        "🧱".blue(),
        format!("{}", block).white().bold(),
        active_transport.cyan(),
        total_connect_ms,
    );

    // ══════════════ PAYLAŞIMLI DURUM ══════════════
    let states: Vec<SharedPoolState> = pools.iter()
        .map(|_| Arc::new(RwLock::new(PoolState::default())))
        .collect();

    // ══════════════ İLK SENKRONİZASYON ══════════════
    println!("\n  {} İlk durum senkronizasyonu yapılıyor...", "🔄".yellow());

    // Bytecode önbelleğe al (bir kez — REVM için)
    let bytecode_results = cache_all_bytecodes(&provider, pools, &states).await;
    for (i, result) in bytecode_results.iter().enumerate() {
        match result {
            Ok(_) => println!("  {}   {} bytecode önbelleğe alındı", "✅".green(), pools[i].name),
            Err(e) => println!("  {}   {} bytecode hatası: {}", "⚠️".yellow(), pools[i].name, e),
        }
    }

    // İlk state sync
    let sync_results = sync_all_pools(&provider, pools, &states, block).await;
    for (i, result) in sync_results.iter().enumerate() {
        match result {
            Ok(_) => {
                let state = states[i].read();
                println!(
                    "  {}   {} → {:.2}$ | Tick: {} | Likidite: {:.2e}",
                    "✅".green(),
                    pools[i].name,
                    state.eth_price_usd,
                    state.tick,
                    state.liquidity_f64,
                );
            }
            Err(e) => println!("  {}   {} state hatası: {}", "❌".red(), pools[i].name, e),
        }
    }

    // ══════════════ İLK TİCKBİTMAP SENKRONİZASYONU ══════════════
    println!("\n  {} TickBitmap derinlik haritası çekiliyor (±{} tick)...", "🗺️".yellow(), config.tick_bitmap_range);
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
            Err(e) => println!("  {}   {} bitmap hatası: {}", "⚠️".yellow(), pools[i].name, e),
        }
    }
    println!("  {} TickBitmap toplam süre: {}ms", "🗺️".cyan(), bitmap_ms);

    // ══════════════ REVM SİMÜLASYON MOTORU ══════════════
    let mut sim_engine = SimulationEngine::new();
    sim_engine.cache_bytecodes(pools, &states);

    // v10.0: Singleton base_db — bytecode bir kez yüklenir, sonra her blokta klonlanır
    {
        let caller_addr = config.private_key.as_ref()
            .and_then(|pk| pk.parse::<alloy::signers::local::PrivateKeySigner>().ok())
            .map(|signer| signer.address())
            .unwrap_or_default();
        let contract_addr = config.contract_address.unwrap_or_default();
        sim_engine.initialize_base_db(pools, &states, caller_addr, contract_addr);
        println!("\n  {} REVM simülasyon motoru hazır (Singleton base_db)", "✅".green());
    }

    // ══════════════ ATOMİK NONCE YÖNETİCİSİ ══════════════
    let executor_address: Option<Address> = config.private_key.as_ref()
        .and_then(|pk| pk.parse::<alloy::signers::local::PrivateKeySigner>().ok())
        .map(|signer| signer.address());

    let nonce_manager = if let Some(address) = executor_address {
        println!("  {} Nonce okunuyor ({})...", "🔢".yellow(), address);
        match provider.get_transaction_count(address).await {
            Ok(nonce) => {
                println!("  {} Başlangıç nonce: {} (RPC'den)", "✅".green(), nonce);
                Arc::new(NonceManager::new(nonce))
            }
            Err(e) => {
                println!("  {} Nonce okunamadı, 0'dan başlanıyor: {}", "⚠️".yellow(), e);
                Arc::new(NonceManager::new(0))
            }
        }
    } else {
        Arc::new(NonceManager::new(0))
    };

    // Execution modu
    if config.execution_enabled() {
        println!(
            "  {} Kontrat tetikleme: {} (Adres: {})",
            "🚀".green(),
            "AKTİF".green().bold(),
            config.contract_address
                .expect("BUG: execution_enabled() true ama contract_address None")
        );
    } else {
        println!(
            "  {} Kontrat tetikleme: {} (Sadece gözlem)",
            "ℹ️".blue(),
            "DEVRE DIŞI".yellow().bold()
        );
    }

    // ══════════════ BLOK BAŞLIĞI ABONELİĞİ ══════════════
    println!();
    println!("{}", "  ════════════════════════════════════════════════════════════════".green());
    println!("  {}  CANLI YAYIN v9.0 — Yeni bloklar + Pending TX dinleniyor...", "📡".green());
    println!("  {}  Döngü: Pending TX → State Sync → TickBitmap → NR → REVM → Yürüt", "📡".green());
    println!("{}", "  ════════════════════════════════════════════════════════════════".green());
    println!();

    // ══════════════ PENDING TX DİNLEYİCİ (FAZ 4) ══════════════
    // Base L2 sequencer'daki bekleyen swap TX'lerini arka planda dinle
    // ve etkilenen havuzların durumlarını iyimser (optimistic) olarak güncelle.
    // Bu sayede blok onayını beklemeden ~15-20ms erken hareket edilir.
    let pool_addresses: Vec<Address> = pools.iter().map(|p| p.address).collect();
    {
        let pools_bg = pools.to_vec();
        let states_bg: Vec<SharedPoolState> = states.iter().map(|s| Arc::clone(s)).collect();
        let pool_addrs_bg = pool_addresses.clone();
        let rpc_url_bg = config.rpc_wss_url.clone();

        tokio::spawn(async move {
            // Pending TX stream — best effort, hata olursa sessizce devam et
            match pending_tx_listener(
                &rpc_url_bg,
                &pools_bg,
                &states_bg,
                &pool_addrs_bg,
            ).await {
                Ok(_) => {}
                Err(e) => {
                    eprintln!(
                        "  {} Pending TX dinleyici hatası (blok bazlı akış devam ediyor): {}",
                        "⚠️", e
                    );
                }
            }
        });
    }

    // ══════════════ SWAP EVENT DİNLEYİCİ (v11.0) ══════════════
    // Havuz swap eventlerini eth_subscribe("logs") ile dinle.
    // Swap eventi sqrtPriceX96, liquidity, tick bilgisini doğrudan içerir —
    // ek RPC çağrısı olmadan state güncellenir (zero-latency).
    {
        let pools_ev = pools.to_vec();
        let states_ev: Vec<SharedPoolState> = states.iter().map(|s| Arc::clone(s)).collect();
        let rpc_url_ev = config.rpc_wss_url.clone();

        tokio::spawn(async move {
            // WebSocket bağlantısı kur
            let ws = WsConnect::new(&rpc_url_ev);
            match ProviderBuilder::new().on_ws(ws).await {
                Ok(ws_provider) => {
                    match state_sync::start_swap_event_listener(
                        &ws_provider,
                        &pools_ev,
                        &states_ev,
                    ).await {
                        Ok(_) => {}
                        Err(e) => {
                            eprintln!(
                                "  {} Swap event dinleyici hatası (blok bazlı akış devam ediyor): {}",
                                "⚠️", e
                            );
                        }
                    }
                }
                Err(e) => {
                    eprintln!(
                        "  {} Swap event WS bağlantı hatası: {}",
                        "⚠️", e
                    );
                }
            }
        });
    }

    let sub = provider.subscribe_blocks().await?;
    let mut stream = sub.into_stream();
    let mut stats = ArbitrageStats::new();
    stats.active_transport = active_transport.to_string();
    let mut last_bitmap_block: u64 = block;

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
                    "  {} WSS stream kapandı — yeniden bağlanılıyor...",
                    "⚠️".yellow()
                );
                return Err(eyre::eyre!("WSS stream kapandı"));
            }
            Err(_) => {
                // 15s timeout — bağlantı muhtemelen koptu
                println!(
                    "  {} WSS heartbeat timeout (15s blok yok) — bağlantı yeniden kurulacak",
                    "💔".red()
                );
                return Err(eyre::eyre!("WSS heartbeat timeout: 15 saniyedir blok gelmedi"));
            }
        };

        let block_start = Instant::now();
        let block_number = block_header.header.number.unwrap_or(0);

        // v10.0: Dinamik timestamp ve base_fee — zincir verisinden
        let block_timestamp = block_header.header.timestamp;
        let block_base_fee = block_header.header.base_fee_per_gas
            .unwrap_or(0) as u64;

        // ── 1. DURUM SENKRONİZASYONU ────────────────────────
        let sync_results = sync_all_pools(&provider, pools, &states, block_number).await;

        let sync_ms = block_start.elapsed().as_millis();

        // Gecikme ölçümü
        stats.update_latency(sync_ms as f64);

        let all_synced = sync_results.iter().all(|r| r.is_ok());

        // Hata raporlama
        for (i, result) in sync_results.iter().enumerate() {
            if let Err(e) = result {
                println!(
                    "  {} [Blok #{}] {} sync hatası: {}",
                    "⚠️".yellow(), block_number, pools[i].name, e
                );
            }
        }

        stats.total_blocks_processed += 1;

        // ── 1.5. TİCKBİTMAP PERİYODİK GÜNCELLEME ──────────
        // Her tick_bitmap_max_age_blocks blokta bir TickBitmap'i güncelle
        let bitmap_age = block_number.saturating_sub(last_bitmap_block);
        if bitmap_age >= config.tick_bitmap_max_age_blocks {
            let bm_start = Instant::now();
            let bm_results = sync_all_tick_bitmaps(
                &provider, pools, &states, block_number, config.tick_bitmap_range,
            ).await;
            let bm_ms = bm_start.elapsed().as_millis();

            let bm_ok = bm_results.iter().filter(|r| r.is_ok()).count();
            if bm_ok > 0 {
                println!(
                    "     {} TickBitmap güncellendi ({}/{} havuz, {}ms)",
                    "🗺️".cyan(), bm_ok, pools.len(), bm_ms,
                );
            }
            stats.tick_bitmap_syncs += 1;
            last_bitmap_block = block_number;
        }

        // ── 2. BLOK + SPREAD BİLGİSİ ───────────────────────
        print_block_update(block_number, pools, &states, sync_ms);
        print_spread_info(pools, &states);

        // ── 3. ARBİTRAJ FIRSATI KONTROLÜ ────────────────────
        if all_synced {
            // v10.1: Circuit Breaker — ardışık başarısızlıkta botu güvenle kapat
            //         30s uyku yerine process::exit(1) çağrılır.
            //         Sebep: 3 ardışık revert = sistemik sorun (kontrat hedef alınmış,
            //         likidite çekilmiş, RPC tutarsızlığı vb.). Uyuyup devam etmek
            //         sadece daha fazla gas yakar.
            //         Eşik: CIRCUIT_BREAKER_THRESHOLD (.env, varsayılan=3)
            if stats.consecutive_failures >= config.circuit_breaker_threshold {
                eprintln!(
                    "\n  {} CIRCUIT BREAKER TETIKLENDI! {} ardışık başarısızlık (eşik: {})",
                    "🛑",
                    stats.consecutive_failures,
                    config.circuit_breaker_threshold,
                );
                eprintln!(
                    "  {} Bot güvenli kapanıyor — manuel müdahale gerekli.",
                    "🛑",
                );
                eprintln!(
                    "  {} Son istatistikler: {} blok, {} fırsat, {} başarısız sim, {} işlem",
                    "📊",
                    stats.total_blocks_processed,
                    stats.total_opportunities,
                    stats.failed_simulations,
                    stats.executed_trades,
                );
                // v13.0: Graceful shutdown — process::exit(1) yerine return Err
                // Tokio runtime temizce kapatılır, WS bağlantıları düzgün kesilir,
                // zeroize drop handler çalışır, nonce state korunur.
                return Err(eyre::eyre!(
                    "Circuit breaker tetiklendi: {} ardışık başarısızlık (eşik: {})",
                    stats.consecutive_failures,
                    config.circuit_breaker_threshold
                ));
            }

            if let Some(opportunity) = check_arbitrage_opportunity(pools, &states, config, block_base_fee) {
                // ── 4. DEĞERLENDİR + SİMÜLE + YÜRÜT ──────────
                evaluate_and_execute(
                    &provider,
                    config,
                    pools,
                    &states,
                    &opportunity,
                    &sim_engine,
                    &mut stats,
                    &nonce_manager,
                    block_timestamp,
                    block_base_fee,
                ).await;
            }
        }

        // ── 5. PERİYODİK İSTATİSTİK ────────────────────────
        if stats.total_blocks_processed % config.stats_interval == 0
            && stats.total_blocks_processed > 0
        {
            print_stats_summary(&stats, &states);
        }

        // ── 6. PERİYODİK NONCE SENKRONİZASYONU (v10.0) ──────
        // Her 50 blokta bir zincirdeki gerçek nonce ile lokal nonce'u karşılaştır.
        // Uyumsuzluk varsa zincir değeri ile düzelt (TX kayıpları veya dış müdahale).
        if stats.total_blocks_processed % 50 == 0
            && stats.total_blocks_processed > 0
        {
            if let Some(addr) = executor_address {
                match provider.get_transaction_count(addr).await {
                    Ok(onchain_nonce) => {
                        let local_nonce = nonce_manager.current();
                        if local_nonce != onchain_nonce {
                            println!(
                                "  {} Nonce uyumsuzluğu tespit edildi: lokal={} zincir={} → düzeltiliyor",
                                "🔄".yellow(), local_nonce, onchain_nonce
                            );
                            nonce_manager.force_set(onchain_nonce);
                        }
                    }
                    Err(e) => {
                        println!("  {} Nonce sync başarısız: {}", "⚠️".yellow(), e);
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
    let provider = ProviderBuilder::new().on_ws(ws).await
        .map_err(|e| eyre::eyre!("Pending TX provider bağlantı hatası: {}", e))?;

    println!("  {} Pending TX dinleyici başlatıldı (optimistic mode)", "🔮".cyan());

    // Pending TX stream — full TX nesneleri ile
    let sub = provider.subscribe_full_pending_transactions().await
        .map_err(|e| eyre::eyre!("Pending TX abonelik hatası: {}", e))?;
    let mut stream = sub.into_stream();

    while let Some(tx) = stream.next().await {
        // TX'in hedef adresi izlenen havuzlardan biri mi?
        let tx_to = tx.to;
        let tx_input = &tx.input;

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
                        "     {} [Pending TX] {} iyimser güncelleme: {:.2}$",
                        "🔮".magenta(),
                        pools[pool_idx].name,
                        state.eth_price_usd,
                    );
                }
                Ok(false) => {} // Fiyat değişmedi, sessiz geç
                Err(e) => {
                    // Hata — sessiz devam et, blok bazlı akış zaten çalışıyor
                    eprintln!(
                        "     {} [Pending TX] {} refresh hatası: {}",
                        "⚠️", pools[pool_idx].name, e
                    );
                }
            }
        }
    }

    Ok(())
}