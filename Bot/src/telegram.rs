// ============================================================================
//  TELEGRAM v1.0 — Asimetrik Telemetri Bildirim Sistemi (Katman 11)
//
//  Özellikler:
//  ✓ Non-blocking mpsc kanal mimarisi (trading pipeline ASLA bloklanmaz)
//  ✓ 3 bildirim katmanı: Alfa (anlık başarı), Shift (6 saatlik), Doomsday (acil)
//  ✓ Telegram Bot API entegrasyonu (MarkdownV2 formatlama)
//  ✓ Rate limiting + exponential backoff retry (3 deneme)
//  ✓ Graceful shutdown (CancellationToken ile)
//  ✓ Sıfır ek bağımlılık (reqwest + serde_json zaten mevcut)
// ============================================================================

use chrono::Local;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

// ─────────────────────────────────────────────────────────────────────────────
// Telegram Yapılandırması
// ─────────────────────────────────────────────────────────────────────────────

/// Telegram Bot yapılandırması (.env'den okunur)
#[derive(Debug, Clone)]
pub struct TelegramConfig {
    /// Bot token (@BotFather'dan alınan)
    pub bot_token: String,
    /// Hedef chat/grup ID
    pub chat_id: String,
    /// Bildirimler aktif mi?
    pub enabled: bool,
    /// Vardiya raporu aralığı (saniye, default: 21600 = 6 saat)
    pub shift_report_secs: u64,
    /// Doomsday bakiye eşiği (ETH, default: 0.05)
    pub balance_warn_eth: f64,
}

// ─────────────────────────────────────────────────────────────────────────────
// Bildirim Mesaj Tipleri
// ─────────────────────────────────────────────────────────────────────────────

/// Telegram'a gönderilebilecek tüm bildirim tipleri
#[allow(dead_code)]
pub enum TelegramMessage {
    // ── Kural 1: Alfa Bildirimi (Anlık Başarı) ──
    AlphaSuccess {
        buy_pool: String,
        sell_pool: String,
        gross_profit_weth: f64,
        gas_cost_weth: f64,
        net_profit_weth: f64,
        latency_ms: f64,
        tx_hash: String,
    },

    // ── Kural 2: Vardiya Raporu (Periyodik Özet) ──
    ShiftReport {
        period_label: String,
        scanned_opportunities: u64,
        attempted_trades: u64,
        successful_trades: u64,
        reverts: u64,
        revert_gas_cost_weth: f64,
        net_period_profit_weth: f64,
        wallet_balance_eth: f64,
        uptime: String,
    },

    // ── Kural 3: Doomsday (Kırmızı Alarm) ──
    DoomsdayAlert {
        error_type: String,
        description: String,
        action_taken: String,
    },

    // ── Sistem Bildirimleri ──
    SystemStartup {
        pool_count: usize,
        transport: String,
        mode: String,
    },

    ConnectionLost {
        error: String,
        retry_count: u32,
    },

    ConnectionRestored {
        downtime_secs: u64,
    },

    HighLatency {
        latency_ms: f64,
        threshold_ms: f64,
        block_number: u64,
    },

    CircuitBreakerTripped {
        pair_name: String,
        consecutive_failures: u32,
        cooldown_blocks: u64,
    },

    NewPoolDiscovered {
        pool_name: String,
        pool_count: usize,
    },

    MaxRetriesExceeded {
        max_retries: u32,
    },

    LowBalance {
        balance_eth: f64,
        threshold_eth: f64,
    },

    NonceDrift {
        local_nonce: u64,
        chain_nonce: u64,
    },
}

// ─────────────────────────────────────────────────────────────────────────────
// Telegram Gönderici (Clone'lanabilir Handle)
// ─────────────────────────────────────────────────────────────────────────────

/// Non-blocking Telegram mesaj gönderici.
///
/// Her modül (strategy, main, executor) bu handle'ı clone'layarak
/// kendi mesajlarını gönderir. `try_send()` ile kanal doluysa
/// mesaj sessizce düşürülür — trading pipeline ASLA bloklanmaz.
#[derive(Clone)]
pub struct TelegramSender {
    tx: mpsc::Sender<TelegramMessage>,
}

impl TelegramSender {
    /// Non-blocking mesaj gönder.
    /// Kanal doluysa mesaj düşürülür — sıfır gecikme garantisi.
    pub fn send(&self, msg: TelegramMessage) {
        let _ = self.tx.try_send(msg);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Telemetri Sayaçları (Vardiya Raporu için RAM Sayaçları)
// ─────────────────────────────────────────────────────────────────────────────

/// Dönem bazlı telemetri sayaçları — vardiya raporu için biriktirilir.
/// Her rapor gönderiminde sıfırlanır.
pub struct TelemetryCounters {
    /// Dönem başlangıcı
    pub period_start: Instant,
    /// Taranan fırsat sayısı (spread > 0.001%)
    pub scanned_opportunities: u64,
    /// Denenen işlem sayısı (simülasyona giren)
    pub attempted_trades: u64,
    /// Başarılı işlem sayısı
    pub successful_trades: u64,
    /// Revert sayısı
    pub reverts: u64,
    /// Revert gas maliyeti (WETH)
    pub revert_gas_cost_weth: f64,
    /// Net dönem kârı (WETH)
    pub net_period_profit_weth: f64,
}

impl TelemetryCounters {
    pub fn new() -> Self {
        Self {
            period_start: Instant::now(),
            scanned_opportunities: 0,
            attempted_trades: 0,
            successful_trades: 0,
            reverts: 0,
            revert_gas_cost_weth: 0.0,
            net_period_profit_weth: 0.0,
        }
    }

    /// Dönem sayaçlarını sıfırla (yeni vardiya başlangıcı)
    pub fn reset(&mut self) {
        self.period_start = Instant::now();
        self.scanned_opportunities = 0;
        self.attempted_trades = 0;
        self.successful_trades = 0;
        self.reverts = 0;
        self.revert_gas_cost_weth = 0.0;
        self.net_period_profit_weth = 0.0;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Arka Plan Servisi
// ─────────────────────────────────────────────────────────────────────────────

/// Telegram arka plan servisini başlat.
///
/// `tokio::spawn` ile arka planda çalışır. mpsc kanalından mesaj alır,
/// formatlar ve Telegram Bot API'ye HTTP POST ile gönderir.
///
/// # Dönüş
/// `TelegramSender` — tüm modüller bu handle üzerinden mesaj gönderir.
pub fn spawn_telegram_service(
    config: TelegramConfig,
    cancel: CancellationToken,
) -> TelegramSender {
    let (tx, mut rx) = mpsc::channel::<TelegramMessage>(256);

    tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    eprintln!("  📡 [Telegram] Service shutting down (CancellationToken)");
                    // Kalan mesajları flush et
                    while let Ok(msg) = rx.try_recv() {
                        let text = format_message(&msg);
                        let _ = send_to_telegram(&client, &config.bot_token, &config.chat_id, &text).await;
                    }
                    break;
                }
                msg = rx.recv() => {
                    match msg {
                        Some(message) => {
                            let text = format_message(&message);
                            if let Err(e) = send_to_telegram(&client, &config.bot_token, &config.chat_id, &text).await {
                                eprintln!("  ⚠️ [Telegram] Send error: {}", e);
                            }
                        }
                        None => {
                            // Tüm sender'lar drop oldu — servis kapanabilir
                            eprintln!("  📡 [Telegram] All senders dropped — service exiting");
                            break;
                        }
                    }
                }
            }
        }
    });

    TelegramSender { tx }
}

// ─────────────────────────────────────────────────────────────────────────────
// Telegram Bot API İletişimi
// ─────────────────────────────────────────────────────────────────────────────

/// Telegram Bot API'ye mesaj gönder (3 kez retry, exponential backoff).
async fn send_to_telegram(
    client: &reqwest::Client,
    bot_token: &str,
    chat_id: &str,
    text: &str,
) -> Result<(), String> {
    let url = format!("https://api.telegram.org/bot{}/sendMessage", bot_token);

    let payload = serde_json::json!({
        "chat_id": chat_id,
        "text": text,
        "parse_mode": "HTML",
        "disable_web_page_preview": true,
    });

    let delays = [1u64, 3, 9]; // Exponential backoff: 1s, 3s, 9s
    let mut last_error = String::new();

    for (attempt, delay) in delays.iter().enumerate() {
        match client.post(&url).json(&payload).send().await {
            Ok(response) => {
                if response.status().is_success() {
                    return Ok(());
                }
                let status = response.status();
                let body = response.text().await.unwrap_or_default();

                // Rate limit (429) — daha uzun bekle
                if status.as_u16() == 429 {
                    eprintln!(
                        "  ⚠️ [Telegram] Rate limited (attempt {}/3) — waiting {}s",
                        attempt + 1, delay * 2
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(delay * 2)).await;
                    last_error = format!("HTTP {} — {}", status, body);
                    continue;
                }

                last_error = format!("HTTP {} — {}", status, body);
            }
            Err(e) => {
                last_error = format!("Request error: {}", e);
            }
        }

        if attempt < 2 {
            tokio::time::sleep(std::time::Duration::from_secs(*delay)).await;
        }
    }

    Err(format!("Telegram send failed after 3 attempts: {}", last_error))
}

// ─────────────────────────────────────────────────────────────────────────────
// Mesaj Formatlama (Türkçe, HTML)
// ─────────────────────────────────────────────────────────────────────────────

/// TelegramMessage'ı insan-okunabilir metin formatına dönüştür.
///
/// HTML parse_mode kullanılır (MarkdownV2'nin escape sorunlarından kaçınmak için).
fn format_message(msg: &TelegramMessage) -> String {
    let ts = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

    match msg {
        // ── Kural 1: Alfa Bildirimi ──
        TelegramMessage::AlphaSuccess {
            buy_pool,
            sell_pool,
            gross_profit_weth,
            gas_cost_weth,
            net_profit_weth,
            latency_ms,
            tx_hash,
        } => {
            // Kaba USD tahmini (ETH ~$3000 varsayımı — gerçek fiyat runtime'da bilinir)
            let net_usd_estimate = net_profit_weth * 3000.0;
            format!(
                "🎯 <b>BASARILI ARB YAKALANDI</b>\n\
                 \n\
                 🔄 Havuz: {} ➔ {}\n\
                 💰 Brut Kar: +{:.6} WETH\n\
                 ⛽ Gas Maliyeti: -{:.6} WETH\n\
                 💵 Net Kar: ~{:.6} WETH (~${:.2})\n\
                 ⏱️ Gecikme: {:.1} ms\n\
                 🔗 TX: <code>{}</code>\n\
                 ⏰ {}\n",
                buy_pool, sell_pool,
                gross_profit_weth,
                gas_cost_weth,
                net_profit_weth, net_usd_estimate,
                latency_ms,
                tx_hash,
                ts,
            )
        }

        // ── Kural 2: Vardiya Raporu ──
        TelegramMessage::ShiftReport {
            period_label,
            scanned_opportunities,
            attempted_trades,
            successful_trades,
            reverts,
            revert_gas_cost_weth,
            net_period_profit_weth,
            wallet_balance_eth,
            uptime,
        } => {
            let net_usd_estimate = net_period_profit_weth * 3000.0;
            let revert_usd = revert_gas_cost_weth * 3000.0;
            format!(
                "📊 <b>SISTEM RAPORU ({})</b>\n\
                 \n\
                 🔍 Taranan Firsat: {}\n\
                 ⚡ Denenen Islem: {}\n\
                 ✅ Basarili: {}\n\
                 ❌ Revert: {}\n\
                 💸 Revert Maliyeti: -{:.6} WETH (~${:.2})\n\
                 🏆 Net Donem Kari: {}{:.6} WETH (~${:.2})\n\
                 🔋 Cuzdan Bakiyesi: {:.4} ETH\n\
                 ⏱️ Uptime: {}\n\
                 ⏰ {}\n",
                period_label,
                scanned_opportunities,
                attempted_trades,
                successful_trades,
                reverts,
                revert_gas_cost_weth, revert_usd,
                if *net_period_profit_weth >= 0.0 { "+" } else { "" },
                net_period_profit_weth, net_usd_estimate,
                wallet_balance_eth,
                uptime,
                ts,
            )
        }

        // ── Kural 3: Doomsday Alarm ──
        TelegramMessage::DoomsdayAlert {
            error_type,
            description,
            action_taken,
        } => {
            format!(
                "🚨 <b>KRITIK HATA - MUDAHALE GEREKLI!</b>\n\
                 \n\
                 ⚠️ Hata: {}\n\
                 📋 Detay: {}\n\
                 🛡️ Aksiyon: {}\n\
                 ⏰ {}\n",
                error_type, description, action_taken, ts,
            )
        }

        // ── Sistem Başlatma ──
        TelegramMessage::SystemStartup {
            pool_count,
            transport,
            mode,
        } => {
            format!(
                "🟢 <b>SISTEM BASLATILDI</b>\n\
                 \n\
                 📡 Transport: {}\n\
                 🏊 Havuz Sayisi: {}\n\
                 ⚙️ Mod: {}\n\
                 ⏰ {}\n",
                transport, pool_count, mode, ts,
            )
        }

        // ── Bağlantı Kopması ──
        TelegramMessage::ConnectionLost { error, retry_count } => {
            format!(
                "🔴 <b>BAGLANTI KOPTU</b>\n\
                 \n\
                 ❌ Hata: {}\n\
                 🔄 Yeniden baglanma denemesi: #{}\n\
                 ⏰ {}\n",
                error, retry_count, ts,
            )
        }

        // ── Bağlantı Kurtarma ──
        TelegramMessage::ConnectionRestored { downtime_secs } => {
            format!(
                "🟢 <b>BAGLANTI KURULDU</b>\n\
                 \n\
                 ⏱️ Kesinti suresi: {}s\n\
                 ✅ Sistem normal calismaya devam ediyor\n\
                 ⏰ {}\n",
                downtime_secs, ts,
            )
        }

        // ── Yüksek Gecikme ──
        TelegramMessage::HighLatency {
            latency_ms,
            threshold_ms,
            block_number,
        } => {
            format!(
                "⚡ <b>GECIKME SPIKE</b>\n\
                 \n\
                 📊 Gecikme: {:.0}ms (esik: {:.0}ms)\n\
                 🧱 Blok: #{}\n\
                 ⏰ {}\n",
                latency_ms, threshold_ms, block_number, ts,
            )
        }

        // ── Circuit Breaker ──
        TelegramMessage::CircuitBreakerTripped {
            pair_name,
            consecutive_failures,
            cooldown_blocks,
        } => {
            format!(
                "🛑 <b>CIRCUIT BREAKER TETIKLENDI</b>\n\
                 \n\
                 🏊 Cift: {}\n\
                 ❌ Ardisik basarisizlik: {}\n\
                 ⏳ Kara liste: {} blok (~{}s)\n\
                 ⏰ {}\n",
                pair_name, consecutive_failures,
                cooldown_blocks, cooldown_blocks * 2,
                ts,
            )
        }

        // ── Yeni Havuz Keşfi ──
        TelegramMessage::NewPoolDiscovered {
            pool_name,
            pool_count,
        } => {
            format!(
                "🔍 <b>YENI HAVUZ KESFEDILDI</b>\n\
                 \n\
                 🏊 Havuz: {}\n\
                 📊 Toplam aktif havuz: {}\n\
                 ⏰ {}\n",
                pool_name, pool_count, ts,
            )
        }

        // ── Max Retry Aşıldı ──
        TelegramMessage::MaxRetriesExceeded { max_retries } => {
            format!(
                "🚨 <b>BOT KAPANIYOR - MAX RETRY ASILDI</b>\n\
                 \n\
                 🛑 Maksimum yeniden baglanma denemesi ({}) asildi\n\
                 ⚠️ Bot kapatiliyor, manuel mudahale gerekli!\n\
                 ⏰ {}\n",
                max_retries, ts,
            )
        }

        // ── Düşük Bakiye Uyarısı ──
        TelegramMessage::LowBalance {
            balance_eth,
            threshold_eth,
        } => {
            format!(
                "🚨 <b>DUSUK BAKIYE - OUT OF GAS RISKI</b>\n\
                 \n\
                 🔋 Mevcut bakiye: {:.6} ETH\n\
                 ⚠️ Esik: {:.4} ETH\n\
                 🛡️ Acil ETH yuklemesi gerekli!\n\
                 ⏰ {}\n",
                balance_eth, threshold_eth, ts,
            )
        }

        // ── Nonce Kayması ──
        TelegramMessage::NonceDrift {
            local_nonce,
            chain_nonce,
        } => {
            format!(
                "🔄 <b>NONCE KAYMASI TESPIT EDILDI</b>\n\
                 \n\
                 📊 Lokal: {} | Zincir: {}\n\
                 ✅ Otomatik duzeltildi\n\
                 ⏰ {}\n",
                local_nonce, chain_nonce, ts,
            )
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Unit Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alpha_message_format() {
        let msg = TelegramMessage::AlphaSuccess {
            buy_pool: "UniV3-WETH/USDC".to_string(),
            sell_pool: "Aero-WETH/USDC".to_string(),
            gross_profit_weth: 0.18,
            gas_cost_weth: 0.00004,
            net_profit_weth: 0.17996,
            latency_ms: 1.2,
            tx_hash: "0xabc123".to_string(),
        };
        let text = format_message(&msg);
        assert!(text.contains("BASARILI ARB YAKALANDI"));
        assert!(text.contains("UniV3-WETH/USDC"));
        assert!(text.contains("Aero-WETH/USDC"));
        assert!(text.contains("0.18"));
        assert!(text.contains("0xabc123"));
    }

    #[test]
    fn test_shift_report_format() {
        let msg = TelegramMessage::ShiftReport {
            period_label: "Son 6 Saat".to_string(),
            scanned_opportunities: 14502,
            attempted_trades: 618,
            successful_trades: 4,
            reverts: 614,
            revert_gas_cost_weth: 0.002047,
            net_period_profit_weth: 0.7166,
            wallet_balance_eth: 4.2,
            uptime: "06:00:00".to_string(),
        };
        let text = format_message(&msg);
        assert!(text.contains("SISTEM RAPORU"));
        assert!(text.contains("14502"));
        assert!(text.contains("618"));
        assert!(text.contains("4.2"));
    }

    #[test]
    fn test_doomsday_format() {
        let msg = TelegramMessage::DoomsdayAlert {
            error_type: "Out of Gas".to_string(),
            description: "Cuzdan bakiyesi 0.05 ETH altina dustu".to_string(),
            action_taken: "Bot guvenli moda alindi, islemler durduruldu".to_string(),
        };
        let text = format_message(&msg);
        assert!(text.contains("KRITIK HATA"));
        assert!(text.contains("Out of Gas"));
        assert!(text.contains("guvenli moda"));
    }

    #[test]
    fn test_sender_nonblocking() {
        // Kanal kapasitesi 1 olan sender — 3 mesaj gönder, sadece 1 kabul edilir
        let (tx, _rx) = mpsc::channel::<TelegramMessage>(1);
        let sender = TelegramSender { tx };

        // İlk mesaj kabul edilmeli
        sender.send(TelegramMessage::SystemStartup {
            pool_count: 4,
            transport: "WSS".to_string(),
            mode: "shadow".to_string(),
        });

        // İkinci mesaj — kanal dolu, sessizce düşürülmeli (panic yok)
        sender.send(TelegramMessage::SystemStartup {
            pool_count: 4,
            transport: "WSS".to_string(),
            mode: "shadow".to_string(),
        });

        // Test başarılı — hiçbir zaman bloklanmadı
    }

    #[test]
    fn test_telemetry_counters_reset() {
        let mut counters = TelemetryCounters::new();
        counters.scanned_opportunities = 100;
        counters.successful_trades = 5;
        counters.reverts = 95;
        counters.net_period_profit_weth = 1.5;

        counters.reset();

        assert_eq!(counters.scanned_opportunities, 0);
        assert_eq!(counters.successful_trades, 0);
        assert_eq!(counters.reverts, 0);
        assert!((counters.net_period_profit_weth - 0.0).abs() < f64::EPSILON);
    }
}
