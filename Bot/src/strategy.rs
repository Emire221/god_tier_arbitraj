// ============================================================================
//  STRATEGY v9.0 — Arbitraj Strateji Motoru + 134-Byte Calldata + Dinamik Fee
//
//  v9.0 Yenilikler:
//  ✓ 134-byte kompakt calldata (kontrat v9.0 uyumlu, deadlineBlock dahil)
//  ✓ Deadline block hesaplama (current_block + config.deadline_blocks)
//  ✓ Dinamik bribe/priority fee modeli (beklenen kârın %25'i)
//  ✓ KeyManager entegrasyonu (raw private key yerine şifreli yönetim)
//
//  v7.0 (korunuyor):
//  ✓ owedToken / receivedToken / minProfit hesaplama
//  ✓ Atomik nonce yönetimi entegrasyonu
//  ✓ TickBitmap-aware Newton-Raphson optimizasyonu
//  ✓ Raw TX gönderi (sol! interface yerine TransactionRequest)
// ============================================================================

use alloy::primitives::{Address, U256};
use alloy::providers::Provider;
use alloy::transports::Transport;
use alloy::network::Ethereum;
use alloy::signers::local::PrivateKeySigner;
use alloy::network::EthereumWallet;
use colored::*;
use chrono::Local;
use std::time::Duration;
use std::io::Write;
use std::sync::Arc;

use crate::types::*;
use crate::math;
use crate::simulator::SimulationEngine;

use zeroize::Zeroize;

// ─────────────────────────────────────────────────────────────────────────────
// Zaman Damgası
// ─────────────────────────────────────────────────────────────────────────────

fn timestamp() -> String {
    Local::now().format("%H:%M:%S%.3f").to_string()
}

// ─────────────────────────────────────────────────────────────────────────────
// Arbitraj Fırsat Tespiti
// ─────────────────────────────────────────────────────────────────────────────

/// Her iki havuzun fiyatlarını karşılaştır ve fırsat varsa tespit et
///
/// Fırsat Koşulları:
///   1. Her iki havuz aktif ve veriler taze
///   2. Fiyat farkı (spread) > minimum eşik
///   3. Newton-Raphson ile hesaplanan kâr > minimum net kâr
pub fn check_arbitrage_opportunity(
    pools: &[PoolConfig],
    states: &[SharedPoolState],
    config: &BotConfig,
    block_base_fee: u64,
) -> Option<ArbitrageOpportunity> {
    if pools.len() < 2 || states.len() < 2 {
        return None;
    }

    // Read lock — çok kısa süreli
    let state_a = states[0].read().clone();
    let state_b = states[1].read().clone();

    // Her iki havuz aktif mi?
    if !state_a.is_active() || !state_b.is_active() {
        return None;
    }

    // Veri tazeliği kontrolü
    if state_a.staleness_ms() > config.max_staleness_ms
        || state_b.staleness_ms() > config.max_staleness_ms
    {
        return None;
    }

    let price_a = state_a.eth_price_usd;
    let price_b = state_b.eth_price_usd;

    // Spread hesapla
    let spread = (price_a - price_b).abs();
    let min_price = price_a.min(price_b);
    let spread_pct = if min_price > 0.0 {
        (spread / min_price) * 100.0
    } else {
        return None;
    };

    // Yön belirleme: Ucuzdan al, pahalıya sat
    let (buy_idx, sell_idx) = if price_a < price_b {
        (0, 1) // A ucuz, B pahalı
    } else {
        (1, 0) // B ucuz, A pahalı
    };

    let buy_state = if buy_idx == 0 { &state_a } else { &state_b };
    let sell_state = if sell_idx == 0 { &state_a } else { &state_b };
    let eth_price_ref = (price_a + price_b) / 2.0;

    // ─── TickBitmap referansları (varsa) ───────────────────────────
    let sell_bitmap = sell_state.tick_bitmap.as_ref();
    let buy_bitmap = buy_state.tick_bitmap.as_ref();

    // ─── Dinamik Gas Cost (v10.0) ─────────────────────────────────
    // Formül: gas_cost = (GAS_ESTIMATE * base_fee) / 1e18 * eth_price
    // Base_fee 0 ise (pre-EIP1559 veya hata) fallback: config.gas_cost_usd
    let dynamic_gas_cost_usd = if block_base_fee > 0 {
        let gas_estimate: u64 = 350_000;
        let gas_cost_eth = (gas_estimate as f64 * block_base_fee as f64) / 1e18;
        let cost = gas_cost_eth * eth_price_ref;
        // Minimum floor: 0.001 USD (sıfır gas cost'u engellemek için)
        cost.max(0.001)
    } else {
        config.gas_cost_usd
    };

    // ─── Newton-Raphson Optimal Miktar Hesaplama ──────────────────
    // v6.0: TickBitmap varsa multi-tick hassasiyetinde, yoksa dampening
    let nr_result = math::find_optimal_amount_with_bitmap(
        sell_state,
        pools[sell_idx].fee_fraction,
        buy_state,
        pools[buy_idx].fee_fraction,
        dynamic_gas_cost_usd,
        config.flash_loan_fee_bps,
        eth_price_ref,
        config.max_trade_size_weth,
        pools[sell_idx].token0_is_weth,
        pools[sell_idx].tick_spacing,
        pools[buy_idx].tick_spacing,
        sell_bitmap,
        buy_bitmap,
    );

    // Kârlı değilse fırsatı atla
    if nr_result.expected_profit < config.min_net_profit_usd || nr_result.optimal_amount <= 0.0 {
        return None;
    }

    Some(ArbitrageOpportunity {
        buy_pool_idx: buy_idx,
        sell_pool_idx: sell_idx,
        optimal_amount_weth: nr_result.optimal_amount,
        expected_profit_usd: nr_result.expected_profit,
        buy_price: buy_state.eth_price_usd,
        sell_price: sell_state.eth_price_usd,
        spread_pct,
        nr_converged: nr_result.converged,
        nr_iterations: nr_result.iterations,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Fırsat Değerlendirme ve Yürütme
// ─────────────────────────────────────────────────────────────────────────────

/// Bulunan arbitraj fırsatını değerlendir, simüle et ve gerekirse yürüt
pub async fn evaluate_and_execute<T: Transport + Clone, P: Provider<T, Ethereum> + Sync>(
    _provider: &P,
    config: &BotConfig,
    pools: &[PoolConfig],
    states: &[SharedPoolState],
    opportunity: &ArbitrageOpportunity,
    sim_engine: &SimulationEngine,
    stats: &mut ArbitrageStats,
    nonce_manager: &Arc<NonceManager>,
    block_timestamp: u64,
    block_base_fee: u64,
) {
    let _buy_pool = &pools[opportunity.buy_pool_idx];
    let _sell_pool = &pools[opportunity.sell_pool_idx];

    // ─── İstatistik Güncelle ─────────────────────────────────────
    stats.total_opportunities += 1;
    if opportunity.spread_pct > stats.max_spread_pct {
        stats.max_spread_pct = opportunity.spread_pct;
    }

    // ─── REVM Simülasyonu ──────────────────────────────────────
    let sim_result = sim_engine.validate_mathematical(
        pools,
        states,
        opportunity.buy_pool_idx,
        opportunity.sell_pool_idx,
        opportunity.optimal_amount_weth,
    );

    // Kontrat adresi varsa tam REVM simülasyonu da yap
    let revm_result = if let Some(contract_addr) = config.contract_address {
        let amount_wei = U256::from((opportunity.optimal_amount_weth * 1e18) as u128);

        // v9.0 Calldata: 134-byte kompakt payload (kontrat v9.0 uyumlu)
        // Yön ve token hesaplama:
        //   buy_pool_idx=0 (UniV3 ucuz): uni=1(oneForZero→WETH al), aero=0(zeroForOne→WETH sat)
        //   buy_pool_idx=1 (Slip ucuz):  uni=0(zeroForOne→USDC al), aero=1(oneForZero→USDC sat)
        let (uni_dir, aero_dir, owed_token, received_token) =
            compute_directions_and_tokens(
                opportunity.buy_pool_idx,
                pools[0].token0_is_weth,
                &config.weth_address,
                &config.usdc_address,
            );

        // v9.0: Deadline block hesapla
        let current_block = states[0].read().last_block;
        let deadline_block = current_block as u32 + config.deadline_blocks;

        let calldata = crate::simulator::encode_compact_calldata(
            pools[0].address,  // pool_a (always UniV3)
            pools[1].address,  // pool_b (always Slipstream)
            owed_token,
            received_token,
            amount_wei,
            uni_dir,
            aero_dir,
            0u128, // REVM simulation — minProfit=0
            deadline_block,
        );

        let caller = config.private_key.as_ref()
            .and_then(|pk| pk.parse::<PrivateKeySigner>().ok())
            .map(|signer| signer.address())
            .unwrap_or_default();

        sim_engine.simulate(
            pools,
            states,
            caller,
            contract_addr,
            calldata,
            U256::ZERO,
            current_block as u64,
            block_timestamp,
            block_base_fee,
        )
    } else {
        sim_result.clone()
    };

    // Dinamik gas: REVM simülasyonundan gelen kesin gas değeri
    let simulated_gas_used = revm_result.gas_used;

    // Simülasyon başarısız → işlemi atla
    if !sim_result.success {
        stats.failed_simulations += 1;
        // v10.0: Circuit breaker — ardışık başarısızlık sayacını artır
        stats.consecutive_failures += 1;
        print_simulation_failure(opportunity, &sim_result, pools);
        return;
    }

    // Simülasyon başarılı → ardışık başarısızlık sayacını sıfırla
    stats.consecutive_failures = 0;

    // ─── KÂRLI FIRSAT RAPORU ─────────────────────────────────
    stats.profitable_opportunities += 1;
    stats.total_potential_profit += opportunity.expected_profit_usd;
    if opportunity.expected_profit_usd > stats.max_profit_usd {
        stats.max_profit_usd = opportunity.expected_profit_usd;
    }

    print_opportunity_report(opportunity, &sim_result, pools, config);

    // ─── KONTRAT TETİKLEME VEYA GÖLGE MOD LOGLAMA ─────────────
    if config.shadow_mode() {
        // ═══ GÖLGE MODU: İşlem atlanır, detaylar loglanır ═══
        println!(
            "  {} {}",
            "👻".yellow(),
            "GÖLGE MODU: İşlem atlandı — detaylar shadow_logs.json'a kaydediliyor".yellow().bold()
        );

        // Shadow log kaydı
        write_shadow_log(
            opportunity,
            &sim_result,
            pools,
            config,
        );
    } else if config.execution_enabled() {
        let rpc_url = config.rpc_wss_url.clone();
        let pk = config.private_key.clone().unwrap();
        let contract_addr = config.contract_address.unwrap();
        let trade_weth = opportunity.optimal_amount_weth;
        let _buy_price = opportunity.buy_price;

        // v9.0: Yön ve token hesaplama
        let (uni_dir, aero_dir, owed_token, received_token) =
            compute_directions_and_tokens(
                opportunity.buy_pool_idx,
                pools[0].token0_is_weth,
                &config.weth_address,
                &config.usdc_address,
            );

        // v9.0: Deadline block hesapla
        let current_block = states[0].read().last_block;
        let deadline_block = current_block as u32 + config.deadline_blocks;

        // v9.0: Dinamik bribe/priority fee hesapla
        // Beklenen kârın bribe_pct yüzdesi builder'a gider
        let bribe_pct = config.bribe_pct;
        let expected_profit_wei = (opportunity.expected_profit_usd / opportunity.sell_price * 1e18) as u128;
        let bribe_wei = ((expected_profit_wei as f64) * bribe_pct) as u128;

        // minProfit hesaplama: exact U256 math ile (USD/float YOK)
        let exact_min_profit = {
            let buy_state = states[opportunity.buy_pool_idx].read();
            let sell_state = states[opportunity.sell_pool_idx].read();
            let amount_wei = U256::from((opportunity.optimal_amount_weth * 1e18) as u128);
            let sell_fee_pips = pools[opportunity.sell_pool_idx].fee_bps * 100;
            let buy_fee_pips = pools[opportunity.buy_pool_idx].fee_bps * 100;
            let (exact_profit, _) = math::exact::compute_exact_arbitrage_profit(
                sell_state.sqrt_price_x96,
                sell_state.liquidity,
                sell_state.tick,
                sell_fee_pips,
                pools[opportunity.sell_pool_idx].tick_spacing,
                sell_state.tick_bitmap.as_ref(),
                buy_state.sqrt_price_x96,
                buy_state.liquidity,
                buy_state.tick,
                buy_fee_pips,
                pools[opportunity.buy_pool_idx].tick_spacing,
                buy_state.tick_bitmap.as_ref(),
                amount_wei,
                pools[0].token0_is_weth,
            );
            exact_profit
        };

        // v10.0: Varlık bazlı dinamik slippage
        let slippage_bps = {
            let buy_state = states[opportunity.buy_pool_idx].read();
            let sell_state = states[opportunity.sell_pool_idx].read();
            determine_slippage_factor_bps(buy_state.liquidity, sell_state.liquidity)
        };
        let min_profit = compute_min_profit_exact(exact_min_profit, slippage_bps);

        // Atomik nonce al
        let nonce = nonce_manager.get_and_increment();
        let nm_clone = Arc::clone(nonce_manager);

        stats.executed_trades += 1;

        let pool_a_addr = pools[0].address;
        let pool_b_addr = pools[1].address;

        // REVM'den gelen kesin gas değerini aktar (sabit 350K yerine)
        let sim_gas = simulated_gas_used;

        tokio::spawn(async move {
            execute_on_chain(
                rpc_url, pk, contract_addr,
                pool_a_addr, pool_b_addr,
                owed_token, received_token,
                trade_weth, uni_dir, aero_dir,
                min_profit, deadline_block,
                bribe_wei,
                sim_gas,
                nonce, nm_clone,
            ).await;
        });
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Gölge Modu (Shadow Mode) — JSON Loglama
// ─────────────────────────────────────────────────────────────────────────────

/// Gölge modunda bulunan fırsatın tüm detaylarını shadow_logs.json dosyasına
/// satır satır (JSON Lines / NDJSON formatında) append eder.
///
/// Bu dosya birkaç gün sonra açılıp:
///   "Bot 1000 fırsat bulmuş, gerçek TX atsaydık toplam 450$ kazanacaktık"
/// analizini yapmak için kullanılır.
fn write_shadow_log(
    opportunity: &ArbitrageOpportunity,
    sim_result: &SimulationResult,
    pools: &[PoolConfig],
    _config: &BotConfig,
) {
    let buy_pool = &pools[opportunity.buy_pool_idx];
    let sell_pool = &pools[opportunity.sell_pool_idx];

    // Kompakt calldata boyutunu hesapla (134 byte)
    let payload_bytes = 134;

    // JSON Lines formatında tek satır
    let log_entry = format!(
        concat!(
            "{{",
            "\"timestamp\":\"{}\",",
            "\"block\":0,",
            "\"buy_pool\":\"{}\",",
            "\"buy_pool_addr\":\"{}\",",
            "\"buy_price\":{:.6},",
            "\"sell_pool\":\"{}\",",
            "\"sell_pool_addr\":\"{}\",",
            "\"sell_price\":{:.6},",
            "\"spread_pct\":{:.6},",
            "\"optimal_amount_weth\":{:.8},",
            "\"expected_profit_usd\":{:.6},",
            "\"nr_converged\":{},",
            "\"nr_iterations\":{},",
            "\"sim_success\":{},",
            "\"sim_gas_used\":{},",
            "\"payload_bytes\":{},",
            "\"mode\":\"shadow\"",
            "}}"
        ),
        Local::now().format("%Y-%m-%dT%H:%M:%S%.3f"),
        buy_pool.name,
        buy_pool.address,
        opportunity.buy_price,
        sell_pool.name,
        sell_pool.address,
        opportunity.sell_price,
        opportunity.spread_pct,
        opportunity.optimal_amount_weth,
        opportunity.expected_profit_usd,
        opportunity.nr_converged,
        opportunity.nr_iterations,
        sim_result.success,
        sim_result.gas_used,
        payload_bytes,
    );

    // Dosyaya append (satır satır)
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("shadow_logs.json")
    {
        Ok(mut file) => {
            if let Err(e) = writeln!(file, "{}", log_entry) {
                eprintln!(
                    "  {} shadow_logs.json yazma hatası: {}",
                    "⚠️".yellow(), e
                );
            }
        }
        Err(e) => {
            eprintln!(
                "  {} shadow_logs.json açma hatası: {}",
                "⚠️".yellow(), e
            );
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Kontrat Tetikleme (Zincir Üzeri) — Kompakt 134-Byte Calldata + Dinamik Fee
// ─────────────────────────────────────────────────────────────────────────────

use alloy::providers::ProviderBuilder;
use alloy::rpc::types::TransactionRequest;

/// Arbitraj kontratını zincir üzerinde tetikle
///
/// v9.0: 134-byte kompakt payload + deadline block + dinamik priority fee:
///   [PoolA(20)] + [PoolB(20)] + [owedToken(20)] + [receivedToken(20)]
///   + [Miktar(32)] + [uniDir(1)] + [aeroDir(1)] + [minProfit(16)]
///   + [deadlineBlock(4)] = 134 byte
async fn execute_on_chain(
    rpc_url: String,
    private_key: String,
    contract_address: Address,
    pool_a: Address,
    pool_b: Address,
    owed_token: Address,
    received_token: Address,
    trade_size_weth: f64,
    uni_direction: u8,
    aero_direction: u8,
    min_profit: u128,
    deadline_block: u32,
    bribe_wei: u128,
    simulated_gas: u64,
    nonce: u64,
    nonce_manager: Arc<NonceManager>,
) {
    println!("\n  {} {}", "🚀".yellow(), "KONTRAT TETİKLEME BAŞLATILDI".yellow().bold());

    // v10.0: Private key güvenli bellek yönetimi
    // İmza sonrası private_key RAM'den silinir (zeroize)
    let mut pk_owned = private_key;
    let result = execute_inner(
        &rpc_url, &pk_owned, contract_address,
        pool_a, pool_b,
        owed_token, received_token,
        trade_size_weth, uni_direction, aero_direction,
        min_profit, deadline_block, bribe_wei, simulated_gas, nonce,
    ).await;

    // İmza tamamlandı — private key bellekten güvenle silinir
    pk_owned.zeroize();

    match result {
        Ok(hash) => {
            println!("  {} TX başarılı: {}", "✅".green(), hash.green().bold());
        }
        Err(e) => {
            // TX başarısız — nonce'u geri al
            nonce_manager.rollback();
            println!("  {} TX hatası (nonce geri alındı): {}", "❌".red(), format!("{}", e).red());
        }
    }
}

/// Kontrat tetikleme iç mantığı — 134-byte kompakt calldata + dinamik priority fee
async fn execute_inner(
    rpc_url: &str,
    private_key: &str,
    contract_address: Address,
    pool_a: Address,
    pool_b: Address,
    owed_token: Address,
    received_token: Address,
    trade_size_weth: f64,
    uni_direction: u8,
    aero_direction: u8,
    min_profit: u128,
    deadline_block: u32,
    bribe_wei: u128,
    simulated_gas: u64,
    nonce: u64,
) -> eyre::Result<String> {
    use alloy::providers::WsConnect;

    let signer: PrivateKeySigner = private_key
        .parse()
        .map_err(|_| eyre::eyre!("Geçersiz private key"))?;
    let wallet = EthereumWallet::from(signer);

    let ws = WsConnect::new(rpc_url);
    let provider = ProviderBuilder::new()
        .with_recommended_fillers()
        .wallet(wallet)
        .on_ws(ws)
        .await
        .map_err(|e| eyre::eyre!("TX provider bağlantı hatası: {}", e))?;

    let amount_in_wei = U256::from((trade_size_weth * 1e18) as u128);

    // ═══ CALLDATA MÜHENDİSLİĞİ: 134-BYTE KOMPAKT PAYLOAD ═══
    let calldata = crate::simulator::encode_compact_calldata(
        pool_a,
        pool_b,
        owed_token,
        received_token,
        amount_in_wei,
        uni_direction,
        aero_direction,
        min_profit,
        deadline_block,
    );

    // Calldata hex logla (debug)
    let calldata_hex = crate::simulator::format_compact_calldata_hex(&calldata);
    println!(
        "  {} Kompakt calldata (134 byte): {}...{}",
        "🔧".cyan(),
        &calldata_hex[..22], // 0x + ilk 10 byte
        &calldata_hex[calldata_hex.len().saturating_sub(10)..], // son 5 byte
    );

    // ═══ DİNAMİK PRİORİTY FEE HESAPLAMA ═══
    // Beklenen kârın bribe_pct yüzdesi yüksek priority fee olarak verilir
    // Base L2 FIFO sequencer: priority fee sıralaması belirler
    // Gas değeri: REVM simülasyonundan gelen kesin değer (sabit 350K DEĞİL)
    let priority_fee_per_gas = if bribe_wei > 0 {
        // REVM'den gelen gerçek gas kullanımı (minimum 100K güvenlik tabanı)
        // v10.0: %10 güvenlik tamponu — REVM simülasyonu bazen %5-10 düşük tahmin eder
        //        Gerçek zincirde state diff, cold storage access vb. ek gas tüketebilir.
        //        Bu tampon bribe hesabının güvenli kalmasını sağlar.
        let gas_with_buffer = ((simulated_gas as f64) * 1.10) as u128;
        let actual_gas: u128 = gas_with_buffer.max(100_000);
        let fee = bribe_wei / actual_gas;
        let fee = fee.max(1_000_000); // Minimum 1 Gwei
        println!(
            "  {} Dinamik Priority Fee: {} Gwei (bribe: {} wei, REVM gas: {} (+10% buffer → {}))",
            "💰".yellow(),
            fee / 1_000_000_000,
            bribe_wei,
            simulated_gas,
            actual_gas,
        );
        Some(fee)
    } else {
        None
    };

    // ═══ GAS LIMIT: REVM SİMÜLASYONU × 1.10 (%10 GÜVENLİK TAMPONU) ═══
    // REVM simülasyonundan gelen gas değerine %10 ek marj eklenir.
    // Sebep: Zincirdeki state, TX'in borsaya ulaşana kadar geçen 2-3ms'de
    // başka bir küçük swap nedeniyle değişebilir → cold storage access,
    // state diff vb. ek gas tüketir. Bu tampon "Out of Gas" hatasını önler.
    let gas_limit_with_buffer = ((simulated_gas as f64) * 1.10) as u64;
    let gas_limit = gas_limit_with_buffer.max(150_000); // Minimum 150K güvenlik tabanı

    // ═══ RAW TX GÖNDERİMİ — ATOMIK NONCE + DİNAMİK FEE + GAS LIMIT ═══
    let mut tx = TransactionRequest::default()
        .to(contract_address)
        .input(calldata.into())
        .nonce(nonce)
        .gas_limit(gas_limit as u128);

    // Dinamik priority fee ayarla (varsa)
    if let Some(pf) = priority_fee_per_gas {
        tx = tx.max_priority_fee_per_gas(pf);
    }

    println!(
        "  {} TX gönderiliyor... (miktar: {:.6} WETH, nonce: {}, deadline: blok #{}, gas_limit: {} (+10%), payload: 134 byte)",
        "📤".yellow(), trade_size_weth, nonce, deadline_block, gas_limit
    );
    let pending = provider.send_transaction(tx)
        .await
        .map_err(|e| eyre::eyre!("TX gönderme hatası: {}", e))?;
    let tx_hash = format!("{:?}", pending.tx_hash());
    println!("  {} TX yayınlandı: {}", "📡".blue(), &tx_hash);

    match tokio::time::timeout(Duration::from_secs(60), pending.get_receipt()).await {
        Ok(Ok(receipt)) => {
            println!(
                "  {} Blok: #{}",
                "✅".green(),
                receipt.block_number.unwrap_or_default()
            );
        }
        Ok(Err(e)) => println!("  {} Onay hatası: {}", "⚠️".yellow(), e),
        Err(_) => println!("  {} Zaman aşımı (60s)", "⏰".yellow()),
    }

    Ok(tx_hash)
}

// ─────────────────────────────────────────────────────────────────────────────
// Yön ve Token Hesaplama Yardımcıları
// ─────────────────────────────────────────────────────────────────────────────

/// Arbitraj yönünden UniV3/Slipstream yönlerini ve token adreslerini hesapla
///
/// # Dönüş: (uni_direction, aero_direction, owed_token, received_token)
///
/// Mantık (token0=WETH, token1=USDC varsayımıyla):
/// - buy_pool_idx=0 (UniV3 ucuz → WETH al): uni=1(oneForZero→WETH), aero=0(zeroForOne→WETH sat)
///   owedToken=USDC, receivedToken=WETH
/// - buy_pool_idx=1 (Slip ucuz → WETH al): uni=0(zeroForOne→USDC al), aero=1(oneForZero→USDC sat)
///   owedToken=WETH, receivedToken=USDC
fn compute_directions_and_tokens(
    buy_pool_idx: usize,
    token0_is_weth: bool,
    weth_address: &Address,
    usdc_address: &Address,
) -> (u8, u8, Address, Address) {
    if token0_is_weth {
        // token0 = WETH, token1 = USDC (Base normal düzen)
        if buy_pool_idx == 0 {
            // UniV3'ten WETH al → oneForZero(1), Slipstream'e WETH sat → zeroForOne(0)
            (1u8, 0u8, *usdc_address, *weth_address) // owe USDC, receive WETH
        } else {
            // UniV3'ten USDC al → zeroForOne(0), Slipstream'e USDC sat → oneForZero(1)
            (0u8, 1u8, *weth_address, *usdc_address) // owe WETH, receive USDC
        }
    } else {
        // token0 = USDC, token1 = WETH (ters düzen)
        if buy_pool_idx == 0 {
            (0u8, 1u8, *weth_address, *usdc_address) // owe WETH, receive USDC
        } else {
            (1u8, 0u8, *usdc_address, *weth_address) // owe USDC, receive WETH
        }
    }
}

/// minProfit hesapla (owedToken cinsinden, uint128 wei)
///
/// math::exact::compute_exact_arbitrage_profit ile hesaplanan
/// exact_profit_wei değerinin dinamik bir yüzdesini minProfit olarak ayarla.
///
/// v10.0: Varlık bazlı dinamik slippage:
///   - Derin likidite (>1e18): %99.9 (sadece 10 bps tolerans)
///   - Orta likidite (>1e16): %99.5 (50 bps tolerans)
///   - Sığ likidite:          %95   (500 bps tolerans, güvenli)
///
/// ÖNEMLİ: Float ve USD çevirisi YOKTUR. Tamamen U256 tam sayı matematik.
fn compute_min_profit_exact(exact_profit_wei: U256, slippage_factor_bps: u64) -> u128 {
    // slippage_factor_bps: 9990 = %99.9, 9950 = %99.5, 9500 = %95
    let min_profit_u256 = (exact_profit_wei * U256::from(slippage_factor_bps)) / U256::from(10_000u64);

    // u128'e sığdır (kontrat uint128 bekler). Overflow durumunda u128::MAX kullan.
    if min_profit_u256 > U256::from(u128::MAX) {
        u128::MAX
    } else {
        min_profit_u256.to::<u128>()
    }
}

/// Havuz likidite derinliğine göre slippage faktörü hesapla (bps cinsinden)
///
/// Mantık:
///   - Derin havuzlar (WETH/USDC, likidite > 1e18) → %99.9 (9990 bps)
///     MEV sandwich fırsatı minimuma iner
///   - Orta derinlik (likidite > 1e16) → %99.5 (9950 bps)
///     Makul güvenlik marjı
///   - Sığ havuzlar (altcoin'ler, düşük likidite) → %95 (9500 bps)
///     Yüksek slippage riski, konservatif yaklaşım
fn determine_slippage_factor_bps(buy_liquidity: u128, sell_liquidity: u128) -> u64 {
    let min_liquidity = buy_liquidity.min(sell_liquidity);

    if min_liquidity >= 1_000_000_000_000_000_000 {
        // >= 1e18 aktif likidite → derin havuz
        9990 // %99.9
    } else if min_liquidity >= 10_000_000_000_000_000 {
        // >= 1e16 aktif likidite → orta derinlik
        9950 // %99.5
    } else {
        // Sığ havuz — konservatif
        9500 // %95
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Terminal Çıktıları
// ─────────────────────────────────────────────────────────────────────────────

/// Simülasyon hatası raporu
fn print_simulation_failure(
    opp: &ArbitrageOpportunity,
    sim: &SimulationResult,
    _pools: &[PoolConfig],
) {
    println!(
        "     {} [{}] REVM Simülasyon BAŞARISIZ | Spread: {:.4}% | Sebep: {}",
        "⚠️".yellow(),
        timestamp().dimmed(),
        opp.spread_pct,
        sim.error.as_deref().unwrap_or("Bilinmiyor").red(),
    );
}

/// Kârlı fırsat raporu
fn print_opportunity_report(
    opp: &ArbitrageOpportunity,
    sim: &SimulationResult,
    pools: &[PoolConfig],
    config: &BotConfig,
) {
    let buy = &pools[opp.buy_pool_idx];
    let sell = &pools[opp.sell_pool_idx];

    println!();
    println!("{}", "  ╔═══════════════════════════════════════════════════════════╗".red().bold());
    println!("{}", "  ║     🚨🚨🚨  KÂRLI ARBİTRAJ FIRSATI  🚨🚨🚨              ║".red().bold());
    println!("{}", "  ╠═══════════════════════════════════════════════════════════╣".red().bold());
    println!("  {}  Zaman            : {}", "║".red(), timestamp().white().bold());
    println!(
        "  {}  Yön              : {} → {}",
        "║".red(),
        format!("{}'dan AL ({:.2}$)", buy.name, opp.buy_price).green().bold(),
        format!("{}'e SAT ({:.2}$)", sell.name, opp.sell_price).red().bold(),
    );
    println!("  {}  Spread           : {:.4}%", "║".red(), opp.spread_pct);
    println!("  {}  ──────────────────────────────────────────────────────", "║".red());
    println!(
        "  {}  Optimal Miktar   : {} WETH (Newton-Raphson: {}i, {})",
        "║".red(),
        format!("{:.6}", opp.optimal_amount_weth).white().bold(),
        opp.nr_iterations,
        if opp.nr_converged { "yakınsadı".green() } else { "yakınsamadı".yellow() },
    );
    println!(
        "  {}  {} NET KÂR       : {:.4}$",
        "║".red(),
        "💰",
        format!("{:.4}", opp.expected_profit_usd).green().bold(),
    );
    println!(
        "  {}  REVM Simülasyon  : {} (Gas: {})",
        "║".red(),
        if sim.success { "BAŞARILI".green().bold() } else { "BAŞARISIZ".red().bold() },
        sim.gas_used,
    );

    if config.execution_enabled() {
        println!(
            "  {}  Durum            : {}",
            "║".red(),
            "🚀 KONTRAT TETİKLENİYOR...".yellow().bold()
        );
    } else if config.shadow_mode() {
        println!(
            "  {}  Durum            : {}",
            "║".red(),
            "👻 GÖLGE MODU — shadow_logs.json'a kaydedildi".yellow().bold()
        );
    } else {
        println!(
            "  {}  Durum            : {}",
            "║".red(),
            "👁 Gözlem Modu (tetikleme devre dışı)".dimmed()
        );
    }
    println!("{}", "  ╚═══════════════════════════════════════════════════════════╝".red().bold());
    println!();
}
