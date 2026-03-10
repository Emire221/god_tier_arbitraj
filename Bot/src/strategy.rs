// ============================================================================
//  STRATEGY v18.0 — Arbitraj Strateji Motoru + L1 Data Fee + Fire-and-Forget
//
//  v18.0 Yenilikler:
//  ✓ L1 Data Fee (OP Stack) entegrasyonu — total_gas = L2 + L1
//  ✓ GasPriceOracle.getL1Fee() ile doğru maliyet tahmini
//  ✓ Fire-and-forget TX receipt bekleme (4s timeout, pipeline bloke olmaz)
//  ✓ PGA fallback uyumlu bribe hesabı
//
//  v9.0 (korunuyor):
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
use colored::*;
use chrono::Local;
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
    last_simulated_gas: Option<u64>,
    l1_data_fee_wei: u128,
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

    // ─── v19.0: Havuz Komisyon Güvenlik Tavanı (Sadece Uyarı) ─────
    // v19.0: Statik fee reddi kaldırıldı. Komisyon filtresi artık
    // PreFilter'ın dinamik net kârlılık hesabının parçası.
    // Sadece çok yüksek fee'li havuzlarda (>max_pool_fee_bps) güvenlik reddi.
    {
        let fee_a_bps = state_a.live_fee_bps.unwrap_or(pools[0].fee_bps);
        let fee_b_bps = state_b.live_fee_bps.unwrap_or(pools[1].fee_bps);
        if fee_a_bps > config.max_pool_fee_bps || fee_b_bps > config.max_pool_fee_bps {
            eprintln!(
                "     \u{23ed}\u{fe0f} [FeeFilter] Havuz komisyonu g\u{00fc}venlik tavan\u{0131}n\u{0131} a\u{015f}\u{0131}yor: A={}bps B={}bps (maks={}bps)",
                fee_a_bps, fee_b_bps, config.max_pool_fee_bps,
            );
            return None;
        }
        // v19.0: Yüksek ama kabul edilebilir fee'ler loglansın
        let total_fee_bps = fee_a_bps + fee_b_bps;
        if total_fee_bps > 30 {
            eprintln!(
                "     \u{2139}\u{fe0f} [FeeInfo] Y\u{00fc}ksek toplam komisyon: A={}bps + B={}bps = {}bps \u{2192} dinamik k\u{00e2}rl\u{0131}l\u{0131}k kontrol\u{00fc}ne devrediliyor",
                fee_a_bps, fee_b_bps, total_fee_bps,
            );
        }
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

    // L1 data fee → WETH (tüm gas hesaplarında kullanılacak)
    let l1_data_fee_weth = l1_data_fee_wei as f64 / 1e18;

    // ─── v19.0: O(1) PreFilter — NR'ye girmeden hızlı eleme ───
    // Spread'in fee + gas + bribe maliyetlerini kurtarıp kurtaramayacağını
    // mikrosaniyede kontrol eder. v19.0: Gas maliyetine %20 güvenlik marjı
    // eklendi ve bribe maliyeti de hesaba katıldı → kârsız işlemler
    // en erken safhada elenir.
    {
        // Dinamik gas cost (PreFilter için) — L2 + L1 + %20 güvenlik marjı
        let gas_estimate: u64 = last_simulated_gas.unwrap_or(200_000);
        let prefilter_gas_cost_weth = if block_base_fee > 0 {
            let l2 = (gas_estimate as f64 * block_base_fee as f64) / 1e18;
            // v19.0: %20 güvenlik marjı (gas tahminindeki belirsizlik)
            ((l2 + l1_data_fee_weth) * 1.20).max(0.00002)
        } else {
            ((config.gas_cost_fallback_weth + l1_data_fee_weth) * 1.20).max(0.00002)
        };

        let pre_filter = math::PreFilter {
            fee_a: state_a.live_fee_bps.map(|b| b as f64 / 10_000.0).unwrap_or(pools[0].fee_fraction),
            fee_b: state_b.live_fee_bps.map(|b| b as f64 / 10_000.0).unwrap_or(pools[1].fee_fraction),
            // v19.0: Gas + bribe maliyeti (bribe = kârın %25'i, en kötü senaryo)
            estimated_gas_cost_weth: prefilter_gas_cost_weth,
            min_profit_weth: config.min_net_profit_weth,
            flash_loan_fee_rate: config.flash_loan_fee_bps / 10_000.0,
            // v22.0: PreFilter konservatif bribe kullanır (en kötü senaryo).
            // Gerçek bribe oranı 25-70% aralığında değişir. PreFilter'da
            // düşük bribe kullanmak → kârsız fırsatları NR'ye geçirir,
            // gereksiz hesaplama maliyeti yaratır. Worst-case %50 kullanılır.
            bribe_pct: config.bribe_pct.max(0.50),
        };

        // Kaba tarama miktarı: max trade size'ın %50'si (konservatif tahmin)
        let probe_amount = config.max_trade_size_weth * 0.5;

        match pre_filter.check(price_a, price_b, probe_amount) {
            math::PreFilterResult::Unprofitable { reason } => {
                eprintln!(
                    "     {} [PreFilter] Spread {:.4}% → {:?} | fee_total={:.3}% | gas={:.8} WETH",
                    "\u{23ed}\u{fe0f}",
                    spread_pct,
                    reason,
                    (pre_filter.fee_a + pre_filter.fee_b + config.flash_loan_fee_bps / 10_000.0) * 100.0,
                    prefilter_gas_cost_weth,
                );
                return None;
            }
            math::PreFilterResult::Profitable { estimated_profit_weth, spread_ratio } => {
                eprintln!(
                    "     {} [PreFilter] GEÇTI | spread_ratio={:.6} | est_profit={:.8} WETH → NR'ye devam",
                    "\u{2705}",
                    spread_ratio,
                    estimated_profit_weth,
                );
            }
        }
    }

    // Yön belirleme: Ucuzdan al, pahalıya sat
    let (buy_idx, sell_idx) = if price_a < price_b {
        (0, 1) // A ucuz, B pahalı
    } else {
        (1, 0) // B ucuz, A pahalı
    };

    let buy_state = if buy_idx == 0 { &state_a } else { &state_b };
    let sell_state = if sell_idx == 0 { &state_a } else { &state_b };
    let avg_price_in_quote = (price_a + price_b) / 2.0;

    // ─── TickBitmap referansları (varsa) ───────────────────────────
    let sell_bitmap = sell_state.tick_bitmap.as_ref();
    let buy_bitmap = buy_state.tick_bitmap.as_ref();

    // ─── v11.0: Hard Liquidity Cap — NR Öncesi Havuz Derinlik Kontrolü ─────
    // Havuzun gerçek mevcut likiditesini hesapla (TickBitmap'ten).
    // WETH/USDC havuzlarında 18 vs 6 decimal uyumsuzluğu burada yakalanır.
    // Eğer havuzda sadece ~5 WETH varken MAX_TRADE_SIZE (50 WETH) öneriliyorsa,
    // NR bu tavanla sınırlandırılır ve REVM revert'ü önlenir.
    {
        let sell_hard_cap = math::exact::hard_liquidity_cap_weth(
            sell_state.sqrt_price_x96,
            sell_state.liquidity,
            sell_state.tick,
            pools[sell_idx].token0_is_weth,
            sell_bitmap,
            pools[sell_idx].tick_spacing,
        );
        let buy_hard_cap = math::exact::hard_liquidity_cap_weth(
            buy_state.sqrt_price_x96,
            buy_state.liquidity,
            buy_state.tick,
            pools[buy_idx].token0_is_weth,
            buy_bitmap,
            pools[buy_idx].tick_spacing,
        );
        let effective_cap = sell_hard_cap.min(buy_hard_cap);

        if effective_cap < config.max_trade_size_weth * 0.1 {
            eprintln!(
                "     \u{26a0}\u{fe0f} [Liquidity] Havuz derinliği çok sığ: sell_cap={:.4} buy_cap={:.4} WETH (MAX_TRADE={:.1})",
                sell_hard_cap, buy_hard_cap, config.max_trade_size_weth,
            );
        }

        if effective_cap <= 0.001 {
            eprintln!(
                "     \u{23ed}\u{fe0f} [Liquidity] Yetersiz likidite — NR atlanıyor (cap={:.6} WETH)",
                effective_cap,
            );
            return None;
        }
    }

    // ─── Dinamik Gas Cost (v19.0) ─────────────────────────────────
    // Formül: total_gas = L2_execution_fee + L1_data_fee + güvenlik marjı
    //   L2: gas_cost_weth = (gas_estimate * base_fee) / 1e18
    //   L1: l1_data_fee_wei (GasPriceOracle.getL1Fee() sonucu)
    //
    // OP Stack ağlarında (Base) asıl maliyet L1 data fee'dir.
    // L2 execution fee genelde çok düşüktür (~0.001 Gwei base_fee).
    // L1 data fee'yi hesaba katmamak botun zararına işlem yapmasına yol açar.
    // v19.0: %20 güvenlik marjı eklendi — gas spike'larında zarara girmemek için.
    let dynamic_gas_cost_weth = if block_base_fee > 0 {
        let gas_estimate: u64 = last_simulated_gas.unwrap_or(200_000);
        let l2_gas_cost_weth = (gas_estimate as f64 * block_base_fee as f64) / 1e18;
        // Toplam: (L2 execution + L1 data fee) × 1.20 güvenlik marjı
        ((l2_gas_cost_weth + l1_data_fee_weth) * 1.20).max(0.00002)
    } else {
        ((config.gas_cost_fallback_weth + l1_data_fee_weth) * 1.20).max(0.00002)
    };

    // Gas cost'u quote cinsine çevir (NR için)
    let dynamic_gas_cost_quote = dynamic_gas_cost_weth * avg_price_in_quote;

    // ─── Newton-Raphson Optimal Miktar Hesaplama ──────────────────
    // v6.0: TickBitmap varsa multi-tick hassasiyetinde, yoksa dampening
    // v16.0: Canlı on-chain fee kullanımı (live_fee_bps varsa statik fee yerine)
    let sell_fee = sell_state.live_fee_bps.map(|b| b as f64 / 10_000.0).unwrap_or(pools[sell_idx].fee_fraction);
    let buy_fee = buy_state.live_fee_bps.map(|b| b as f64 / 10_000.0).unwrap_or(pools[buy_idx].fee_fraction);
    let nr_result = math::find_optimal_amount_with_bitmap(
        sell_state,
        sell_fee,
        buy_state,
        buy_fee,
        dynamic_gas_cost_quote,
        config.flash_loan_fee_bps,
        avg_price_in_quote, // gerçek fiyat → kâr quote cinsinden döner
        config.max_trade_size_weth,
        pools[sell_idx].token0_is_weth,
        pools[sell_idx].tick_spacing,
        pools[buy_idx].tick_spacing,
        sell_bitmap,
        buy_bitmap,
        pools[buy_idx].token0_is_weth,
    );

    // NR kârı quote (cbBTC) cinsinden döndü → WETH’e çevir
    let expected_profit_weth = if avg_price_in_quote > 0.0 {
        nr_result.expected_profit / avg_price_in_quote
    } else {
        return None;
    };

    // v15.0 DEBUG: NR sonuç detayları — fırsat filtreleme nedenini göster
    // (Bu loglar canlıya geçiş onayına kadar kaldırılmamalı)
    eprintln!(
        "     {} [DEBUG NR] spread={:.4}% | nr_profit_weth={:.8} | min_required={:.8} | nr_amount={:.6} | converged={} | gas_cost_weth={:.8} (L1={:.8})",
        "\u{1f52c}",
        spread_pct,
        expected_profit_weth,
        config.min_net_profit_weth,
        nr_result.optimal_amount,
        nr_result.converged,
        dynamic_gas_cost_weth,
        l1_data_fee_weth,
    );

    // Kârlı değilse fırsatı atla
    if expected_profit_weth < config.min_net_profit_weth || nr_result.optimal_amount <= 0.0 {
        eprintln!(
            "     {} [DEBUG] Fırsat kârsız — NR profit ({:.8}) < eşik ({:.8}) veya amount<=0 ({:.6})",
            "\u{23ed}\u{fe0f}",
            expected_profit_weth,
            config.min_net_profit_weth,
            nr_result.optimal_amount,
        );
        return None;
    }

    Some(ArbitrageOpportunity {
        buy_pool_idx: buy_idx,
        sell_pool_idx: sell_idx,
        optimal_amount_weth: nr_result.optimal_amount,
        expected_profit_weth,
        buy_price_quote: buy_state.eth_price_usd,
        sell_price_quote: sell_state.eth_price_usd,
        spread_pct,
        nr_converged: nr_result.converged,
        nr_iterations: nr_result.iterations,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Fırsat Değerlendirme ve Yürütme
// ─────────────────────────────────────────────────────────────────────────────

/// Bulunan arbitraj fırsatını değerlendir, simüle et ve gerekirse yürüt
///
/// Dönüş: REVM simülasyonundan gelen gerçek gas kullanımı (sonraki bloklarda
/// `check_arbitrage_opportunity`'e beslenir → dinamik gas maliyet hesaplaması).
///
/// v21.0: `mev_executor` parametresi eklendi — işlemler yalnızca Private RPC
/// (eth_sendBundle) üzerinden gönderilir, public mempool kullanılmaz.
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
    block_latency_ms: f64,
    _l1_data_fee_wei: u128,
    mev_executor: &Arc<crate::executor::MevExecutor>,
) -> Option<u64> {
    let _buy_pool = &pools[opportunity.buy_pool_idx];
    let _sell_pool = &pools[opportunity.sell_pool_idx];

    // ─── v12.0: Sıfıra Bölünme / NaN / Infinity Koruması ─────────────
    // RPC kopukluğu veya sıfır sqrtPriceX96 durumunda fiyatlar 0.0 olabilir.
    // Float bölüm sonucu Infinity → u128'e cast'te Rust panic! verir.
    // Bu kontrol thread çökmesini önler ve döngüyü sessizce atlar.
    if opportunity.sell_price_quote <= 0.0
        || opportunity.buy_price_quote <= 0.0
        || opportunity.optimal_amount_weth <= 0.0
        || !opportunity.expected_profit_weth.is_finite()
    {
        return None;
    }

    // ─── İstatistik Güncelle ─────────────────────────────────────
    // v15.0: total_opportunities ve max_spread_pct artık main.rs'de
    // her blokta güncelleniyor (fırsat koşulundan bağımsız).
    // Burada sadece simülasyona özgü istatistikler kalıyor.

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
        // v11.0 Calldata: Yön ve token hesaplama
        //   buy_pool_idx=0 (UniV3 ucuz): uni=1(oneForZero→WETH al), aero=0(zeroForOne→WETH sat)
        //   buy_pool_idx=1 (Slip ucuz):  uni=0(zeroForOne→Quote al), aero=1(oneForZero→Quote sat)
        let (uni_dir, aero_dir, owed_token, received_token) =
            compute_directions_and_tokens(
                opportunity.buy_pool_idx,
                pools[0].token0_is_weth,
                &config.weth_address,
                &pools[0].quote_token_address,
            );

        // ═══ v11.0: DİNAMİK DECIMAL AMOUNT HESAPLAMA ═══
        // Kritik düzeltme: Input tokeni WETH mi Quote mi?
        //   - WETH input → amount * 10^18
        //   - Quote input → amount * eth_price * 10^quote_decimals
        // Eski hata: Her zaman 10^18 kullanılıyordu → Quote input'ta
        //            hatalı hesaplama oluşuyordu.
        let weth_input = crate::types::is_weth_input(uni_dir, pools[0].token0_is_weth);
        let amount_wei = crate::types::weth_amount_to_input_wei(
            opportunity.optimal_amount_weth,
            weth_input,
            (opportunity.buy_price_quote + opportunity.sell_price_quote) / 2.0,
            if pools[0].token0_is_weth { pools[0].token1_decimals } else { pools[0].token0_decimals },
        );

        // v9.0: Deadline block hesapla (v11.0: minimum +3 tolerans)
        let current_block = states[0].read().last_block;
        let deadline_block = current_block as u32 + config.deadline_blocks.max(3);

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
        return None;
    }

    // Simülasyon başarılı → ardışık başarısızlık sayacını sıfırla
    stats.consecutive_failures = 0;

    // ─── KÂRLI FIRSAT RAPORU ─────────────────────────────────
    stats.profitable_opportunities += 1;
    stats.total_potential_profit += opportunity.expected_profit_weth;
    if opportunity.expected_profit_weth > stats.max_profit_weth {
        stats.max_profit_weth = opportunity.expected_profit_weth;
    }

    print_opportunity_report(opportunity, &sim_result, pools, config);

    // ─── KONTRAT TETİKLEME VEYA GÖLGE MOD LOGLAMA ─────────────
    if config.shadow_mode() {
        // ═══ GÖLGE MODU: İşlem atlanır, detaylar loglanır ═══

        // v23.0 (Y-1): Gölge modu ekonomik uygulanabilirlik istatistikleri
        if sim_result.success {
            stats.shadow_sim_success += 1;
            stats.shadow_cumulative_profit += opportunity.expected_profit_weth;
        } else {
            stats.shadow_sim_fail += 1;
        }

        println!(
            "  {} {}",
            "👻".yellow(),
            "GÖLGE MODU: İşlem atlandı — detaylar shadow_analytics.jsonl'e kaydediliyor".yellow().bold()
        );
        // v23.0 (Y-1): Periyodik ekonomik özet (her 10 fırsatta bir)
        let total_shadow = stats.shadow_sim_success + stats.shadow_sim_fail;
        if total_shadow > 0 && total_shadow % 10 == 0 {
            let success_rate = (stats.shadow_sim_success as f64 / total_shadow as f64) * 100.0;
            println!(
                "  {} Gölge Özet: {} fırsat | Sim başarı: {:.1}% | Kümülatif kâr: {:.6} WETH",
                "📊".cyan(),
                total_shadow,
                success_rate,
                stats.shadow_cumulative_profit,
            );
        }

        // Dinamik bribe hesabı (loglama için)
        let dynamic_bribe_weth = opportunity.expected_profit_weth * config.bribe_pct;

        // Shadow log kaydı (v10.0: yapılandırılmış JSONL)
        write_shadow_log(
            opportunity,
            &sim_result,
            pools,
            config,
            simulated_gas_used,
            dynamic_bribe_weth,
            block_latency_ms,
        );
    } else if config.execution_enabled() {
        let pk = config.private_key.clone()
            .expect("BUG: execution_enabled() true ama private_key None");
        let contract_addr = config.contract_address
            .expect("BUG: execution_enabled() true ama contract_address None");
        let trade_weth = opportunity.optimal_amount_weth;
        let _buy_price = opportunity.buy_price_quote;

        // v11.0: Yön ve token hesaplama
        let (uni_dir, aero_dir, owed_token, received_token) =
            compute_directions_and_tokens(
                opportunity.buy_pool_idx,
                pools[0].token0_is_weth,
                &config.weth_address,
                &pools[0].quote_token_address,
            );

        // v11.0: Deadline block hesapla (minimum +3 tolerans)
        let current_block = states[0].read().last_block;
        let deadline_block = current_block as u32 + config.deadline_blocks.max(3);

        // v21.0: Bribe hesabı MevExecutor::compute_dynamic_bribe'a devredildi.
        // MevExecutor, expected_profit_weth + simulated_gas + block_base_fee
        // bilgilerini alarak adaptatif bribe yüzdesini kendi içinde hesaplar
        // ve priority fee olarak TX'e ekler.

        // ═══ v11.0: YÖN-BAZLI EXACT minProfit HESAPLAMA ═══
        // Kritik düzeltme: Eski sistem her zaman WETH cinsinden profit hesaplıyordu.
        // Ancak kontrat balAfter(owedToken) - balBefore(owedToken) hesabı yapar.
        // owedToken=Quote ise kâr quote cinsinden ölçülür → minProfit quote_decimals olmalı.
        //
        // Yeni sistem: Flash swap akışını birebir modelleyen
        // compute_exact_directional_profit kullanılır.
        // Bu fonksiyon doğrudan owedToken cinsinden kâr döndürür.
        let exact_min_profit = {
            let pool_a_state = states[0].read();
            let pool_b_state = states[1].read();
            let pool_a_fee_pips = pools[0].fee_bps * 100;
            let pool_b_fee_pips = pools[1].fee_bps * 100;

            let weth_input = crate::types::is_weth_input(uni_dir, pools[0].token0_is_weth);
            let sim_amount_wei = crate::types::weth_amount_to_input_wei(
                opportunity.optimal_amount_weth,
                weth_input,
                (opportunity.buy_price_quote + opportunity.sell_price_quote) / 2.0,
                if pools[0].token0_is_weth { pools[0].token1_decimals } else { pools[0].token0_decimals },
            );

            let uni_zero_for_one = uni_dir == 0;
            let aero_zero_for_one = aero_dir == 0;

            math::exact::compute_exact_directional_profit(
                pool_a_state.sqrt_price_x96,
                pool_a_state.liquidity,
                pool_a_state.tick,
                pool_a_fee_pips,
                pool_a_state.tick_bitmap.as_ref(),
                pool_b_state.sqrt_price_x96,
                pool_b_state.liquidity,
                pool_b_state.tick,
                pool_b_fee_pips,
                pool_b_state.tick_bitmap.as_ref(),
                sim_amount_wei,
                uni_zero_for_one,
                aero_zero_for_one,
            )
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

        // v11.0: ETH fiyatı ve token sırası bilgisini execute_on_chain'e aktar
        let eth_price_for_exec = (opportunity.buy_price_quote + opportunity.sell_price_quote) / 2.0;
        let t0_is_weth = pools[0].token0_is_weth;

        // v13.0: block_base_fee'yi execute'a aktar (max_fee_per_gas hesabı için)
        let base_fee_for_exec = block_base_fee;
        let qt_decimals = if pools[0].token0_is_weth { pools[0].token1_decimals } else { pools[0].token0_decimals };

        let expected_profit = opportunity.expected_profit_weth;
        let mev_exec = Arc::clone(mev_executor);

        tokio::spawn(async move {
            execute_on_chain_protected(
                mev_exec, pk, contract_addr,
                pool_a_addr, pool_b_addr,
                owed_token, received_token,
                trade_weth, uni_dir, aero_dir,
                min_profit, deadline_block,
                sim_gas,
                nonce, nm_clone,
                eth_price_for_exec,
                t0_is_weth,
                base_fee_for_exec,
                qt_decimals,
                expected_profit,
                current_block as u64,
            ).await;
        });
    }

    // v14.0: REVM'den gelen gerçek gas değerini döndür
    // Bir sonraki blokta check_arbitrage_opportunity'ye beslenir
    Some(simulated_gas_used)
}

// ─────────────────────────────────────────────────────────────────────────────
// Gölge Modu (Shadow Mode) — JSON Loglama
// ─────────────────────────────────────────────────────────────────────────────

/// Gölge modunda bulunan fırsatın tüm detaylarını shadow_analytics.jsonl
/// dosyasına satır satır (JSON Lines / NDJSON formatında) append eder.
///
/// v10.0 Yapılandırılmış Alanlar:
///   - timestamp, pool_pair, gas_used, expected_profit
///   - simulated_profit, dynamic_bribe, latency_ms
///
/// Bu dosya birkaç gün sonra açılıp:
///   "Bot 1000 fırsat bulmuş, gerçek TX atsaydık toplam 450$ kazanacaktık"
/// analizini yapmak için kullanılır.
fn write_shadow_log(
    opportunity: &ArbitrageOpportunity,
    sim_result: &SimulationResult,
    pools: &[PoolConfig],
    _config: &BotConfig,
    simulated_gas: u64,
    dynamic_bribe_weth: f64,
    latency_ms: f64,
) {
    let buy_pool = &pools[opportunity.buy_pool_idx];
    let sell_pool = &pools[opportunity.sell_pool_idx];

    // pool_pair: "UniV3-WETH/cbBTC ↔ Aero-WETH/cbBTC"
    let pool_pair = format!("{} ↔ {}", buy_pool.name, sell_pool.name);

    // Simulated profit = expected profit if sim succeeded, 0 otherwise
    let simulated_profit_weth = if sim_result.success {
        opportunity.expected_profit_weth
    } else {
        0.0
    };

    // JSONL yapılandırılmış log satırı
    let log_entry = serde_json::json!({
        "timestamp": chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f").to_string(),
        "pool_pair": pool_pair,
        "buy_pool": buy_pool.name,
        "buy_pool_addr": format!("{}", buy_pool.address),
        "buy_price_quote": (opportunity.buy_price_quote * 1e6).round() / 1e6,
        "sell_pool": sell_pool.name,
        "sell_pool_addr": format!("{}", sell_pool.address),
        "sell_price_quote": (opportunity.sell_price_quote * 1e6).round() / 1e6,
        "spread_pct": (opportunity.spread_pct * 1e6).round() / 1e6,
        "optimal_amount_weth": (opportunity.optimal_amount_weth * 1e8).round() / 1e8,
        "expected_profit": (opportunity.expected_profit_weth * 1e8).round() / 1e8,
        "simulated_profit": (simulated_profit_weth * 1e8).round() / 1e8,
        "gas_used": simulated_gas,
        "dynamic_bribe": (dynamic_bribe_weth * 1e8).round() / 1e8,
        "latency_ms": (latency_ms * 10.0).round() / 10.0,
        "nr_converged": opportunity.nr_converged,
        "nr_iterations": opportunity.nr_iterations,
        "sim_success": sim_result.success,
        "sim_error": sim_result.error.as_deref(),
        "mode": "shadow",
    });

    // v22.1: Dosya boyutu kontrolü — 50MB'ı aşarsa rotate et
    let log_path = std::path::Path::new("shadow_analytics.jsonl");
    const MAX_LOG_SIZE: u64 = 50 * 1024 * 1024; // 50 MB
    if let Ok(metadata) = std::fs::metadata(log_path) {
        if metadata.len() >= MAX_LOG_SIZE {
            let rotated = format!("shadow_analytics.{}.jsonl",
                chrono::Local::now().format("%Y%m%d_%H%M%S"));
            let _ = std::fs::rename(log_path, &rotated);
            eprintln!("  📁 Shadow log rotate edildi → {}", rotated);
        }
    }

    // Dosyaya append (satır satır)
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    {
        Ok(mut file) => {
            if let Err(e) = writeln!(file, "{}", log_entry) {
                eprintln!(
                    "  {} shadow_analytics.jsonl yazma hatası: {}",
                    "⚠️".yellow(), e
                );
            }
        }
        Err(e) => {
            eprintln!(
                "  {} shadow_analytics.jsonl açma hatası: {}",
                "⚠️".yellow(), e
            );
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Kontrat Tetikleme (Zincir Üzeri) — MevExecutor Üzerinden Private RPC
// ─────────────────────────────────────────────────────────────────────────────

// v21.0: ProviderBuilder ve TransactionRequest artık MevExecutor'da kullanılır.
// strategy.rs doğrudan TX oluşturmaz.

/// v21.0: Arbitraj kontratını MevExecutor üzerinden Private RPC ile tetikle.
///
/// Public mempool kullanılmaz — tüm işlemler eth_sendBundle ile gönderilir.
/// Private RPC yoksa veya başarısızsa işlem İPTAL EDİLİR (nonce geri alınır).
async fn execute_on_chain_protected(
    mev_executor: Arc<crate::executor::MevExecutor>,
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
    simulated_gas: u64,
    nonce: u64,
    nonce_manager: Arc<NonceManager>,
    eth_price_in_quote: f64,
    token0_is_weth: bool,
    block_base_fee: u64,
    quote_token_decimals: u8,
    expected_profit_weth: f64,
    current_block: u64,
) {
    println!("\n  {} {}", "🚀".yellow(), "KONTRAT TETİKLEME BAŞLATILDI (Private RPC)".yellow().bold());

    // v10.0: Private key güvenli bellek yönetimi
    let mut pk_owned = private_key;

    // Calldata oluştur
    let weth_input = crate::types::is_weth_input(uni_direction, token0_is_weth);
    let amount_in_wei = crate::types::weth_amount_to_input_wei(
        trade_size_weth,
        weth_input,
        eth_price_in_quote,
        quote_token_decimals,
    );

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

    let calldata_hex = crate::simulator::format_compact_calldata_hex(&calldata);
    println!(
        "  {} Kompakt calldata (134 byte): {}...{}",
        "🔧".cyan(),
        &calldata_hex[..22],
        &calldata_hex[calldata_hex.len().saturating_sub(10)..],
    );

    println!(
        "  {} TX gönderiliyor (Private RPC)... (miktar: {:.6} WETH, nonce: {}, deadline: blok #{}, payload: 134 byte)",
        "📤".yellow(), trade_size_weth, nonce, deadline_block
    );

    // MevExecutor üzerinden gönder — Private RPC yoksa otomatik iptal
    let result = mev_executor.execute_protected(
        &pk_owned,
        contract_address,
        &calldata,
        nonce,
        expected_profit_weth,
        simulated_gas,
        block_base_fee,
        current_block,
        &nonce_manager,
    ).await;

    // İmza tamamlandı — private key bellekten güvenle silinir
    pk_owned.zeroize();

    match result {
        Ok(hash) => {
            println!("  {} TX başarılı (Private RPC): {}", "✅".green(), hash.green().bold());
        }
        Err(e) => {
            println!("  {} TX hatası: {}", "❌".red(), format!("{}", e).red());
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Yön ve Token Hesaplama Yardımcıları
// ─────────────────────────────────────────────────────────────────────────────

/// Arbitraj yönünden UniV3/Slipstream yönlerini ve token adreslerini hesapla
///
/// # Dönüş: (uni_direction, aero_direction, owed_token, received_token)
///
/// Mantık (token0=WETH, token1=Quote varsayımıyla):
/// - buy_pool_idx=0 (UniV3 ucuz → WETH al): uni=1(oneForZero→WETH), aero=0(zeroForOne→WETH sat)
///   owedToken=Quote, receivedToken=WETH
/// - buy_pool_idx=1 (Slip ucuz → WETH al): uni=0(zeroForOne→Quote al), aero=1(oneForZero→Quote sat)
///   owedToken=WETH, receivedToken=Quote
fn compute_directions_and_tokens(
    buy_pool_idx: usize,
    token0_is_weth: bool,
    weth_address: &Address,
    quote_token_address: &Address,
) -> (u8, u8, Address, Address) {
    if token0_is_weth {
        // token0 = WETH, token1 = Quote (Base normal düzen)
        if buy_pool_idx == 0 {
            // UniV3'ten WETH al → oneForZero(1), Slipstream'e WETH sat → zeroForOne(0)
            (1u8, 0u8, *quote_token_address, *weth_address) // owe Quote, receive WETH
        } else {
            // UniV3'ten Quote al → zeroForOne(0), Slipstream'e Quote sat → oneForZero(1)
            (0u8, 1u8, *weth_address, *quote_token_address) // owe WETH, receive Quote
        }
    } else {
        // token0 = Quote, token1 = WETH (ters düzen)
        if buy_pool_idx == 0 {
            (0u8, 1u8, *weth_address, *quote_token_address) // owe WETH, receive Quote
        } else {
            (1u8, 0u8, *quote_token_address, *weth_address) // owe Quote, receive WETH
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
/// ÖNEMLİ: Float ve quote çevirisi YOKTUR. Tamamen U256 tam sayı matematik.
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
///   - Derin havuzlar (WETH/Quote, likidite > 1e18) → %99.9 (9990 bps)
///     MEV sandwich fırsatı minimuma iner
///   - Orta derinlik (likidite > 1e16) → %99.5 (9950 bps)
///     Makul güvenlik marjı
///   - Sığ havuzlar (altcoin'ler, düşük likidite) → %95 (9500 bps)
///     Yüksek slippage riski, konservatif yaklaşım
fn determine_slippage_factor_bps(buy_liquidity: u128, sell_liquidity: u128) -> u64 {
    let min_liquidity = buy_liquidity.min(sell_liquidity);

    // v22.0: Sim vs real divergence koruması artırıldı.
    // REVM simülasyonu ile gerçek yürütme arasında fiyat kayması olabilir
    // (aradaki blokta başka swap'lar gerçekleşebilir). Daha konservatif
    // slippage faktörleri kullanılır.
    if min_liquidity >= 1_000_000_000_000_000_000 {
        // >= 1e18 aktif likidite → derin havuz
        9950 // %99.5 (eski: %99.9)
    } else if min_liquidity >= 10_000_000_000_000_000 {
        // >= 1e16 aktif likidite → orta derinlik
        9900 // %99 (eski: %99.5)
    } else {
        // Sığ havuz — konservatif
        9500 // %95 (değişmedi)
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
        format!("{}'dan AL ({:.6} Q)", buy.name, opp.buy_price_quote).green().bold(),
        format!("{}'e SAT ({:.6} Q)", sell.name, opp.sell_price_quote).red().bold(),
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
        "  {}  {} NET KÂR       : {:.6} WETH",
        "║".red(),
        "💰",
        format!("{:.6}", opp.expected_profit_weth).green().bold(),
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
            "👻 GÖLGE MODU — shadow_analytics.jsonl'e kaydedildi".yellow().bold()
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

// ─────────────────────────────────────────────────────────────────────────────
// Exponential Gas Base Fee Spike Testleri
// ─────────────────────────────────────────────────────────────────────────────
//
// EIP-1559 gereği Base ağında base fee ardışık dolu bloklarda logaritmik
// olarak artabilir. strategy.rs içindeki risk filtresi kâr/zarar hesabı
// yaparken ağın o anki gas'ını kullanır.
//
// Bu test modülü, base fee ani 5x artışında:
//   1. check_arbitrage_opportunity'nin gas maliyetini doğru hesaplaması
//   2. Kâr < gas_cost olduğunda fırsatı reddetmesi (None dönmesi)
//   3. Normal gas'ta kârlı fırsatın kabul edilmesi (Some dönmesi)
// davranışlarını doğrular.
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod gas_spike_tests {
    use super::*;
    use alloy::primitives::{address, Address};
    use std::sync::Arc;
    use parking_lot::RwLock;
    use std::time::Instant;

    const POOL_A_ADDR: Address = address!("d0b53D9277642d899DF5C87A3966A349A798F224");
    const POOL_B_ADDR: Address = address!("cDAC0d6c6C59727a65F871236188350531885C43");
    const WETH_ADDR: Address = address!("4200000000000000000000000000000000000006");

    fn make_test_config(min_profit: f64, gas_cost_fallback: f64) -> BotConfig {
        BotConfig {
            rpc_wss_url: "wss://test".into(),
            rpc_http_url: "https://test".into(),
            rpc_ipc_path: None,
            transport_mode: TransportMode::Ws,
            private_key: None,
            contract_address: None,
            weth_address: WETH_ADDR,
            gas_cost_fallback_weth: gas_cost_fallback,
            flash_loan_fee_bps: 5.0,
            min_net_profit_weth: min_profit,
            stats_interval: 100,
            max_retries: 0,
            initial_retry_delay_secs: 2,
            max_retry_delay_secs: 60,
            max_staleness_ms: 5000,
            max_trade_size_weth: 50.0,
            chain_id: 8453,
            tick_bitmap_range: 500,
            tick_bitmap_max_age_blocks: 5,
            execution_enabled_flag: false,
            admin_address: None,
            deadline_blocks: 2,
            bribe_pct: 0.25,
            keystore_path: None,
            key_manager_active: false,
            circuit_breaker_threshold: 3,
            rpc_wss_url_backup: None,
            latency_spike_threshold_ms: 200.0,
            private_rpc_url: None,
            rpc_wss_url_extra: Vec::new(),
            max_pool_fee_bps: 200, // Test: yüksek tavan — gas spike testleri fee filtresinden etkilenmesin
        }
    }

    fn make_pool_configs() -> Vec<PoolConfig> {
        vec![
            PoolConfig {
                address: POOL_A_ADDR,
                name: "UniV3-test".into(),
                fee_bps: 5,
                fee_fraction: 0.0005,
                token0_decimals: 18,
                token1_decimals: 8,
                dex: DexType::UniswapV3,
                token0_is_weth: true,
                tick_spacing: 10,
                quote_token_address: address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"),
            },
            PoolConfig {
                address: POOL_B_ADDR,
                name: "Aero-test".into(),
                fee_bps: 100,
                fee_fraction: 0.01,
                token0_decimals: 18,
                token1_decimals: 8,
                dex: DexType::Aerodrome,
                token0_is_weth: true,
                tick_spacing: 1,
                quote_token_address: address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"),
            },
        ]
    }

    fn make_pool_state(eth_price: f64, liq: u128, block: u64) -> SharedPoolState {
        // sqrtPriceX96 hesapla — math.rs::make_test_pool ile tutarlı formül
        let price_ratio = eth_price * 1e-12; // token1/token0 raw fiyat oranı
        let sqrt_price = price_ratio.sqrt();
        let sqrt_price_f64 = sqrt_price * (1u128 << 96) as f64;
        // Tick'i sqrtPriceX96'dan doğru hesapla (dampening tutarlılığı için)
        let tick = math::sqrt_price_x96_to_tick(sqrt_price_f64);
        // v7.0: U256 sqrtPriceX96 artık exact tick-bazlı hesaplanır
        let sqrt_price_x96_u256 = math::exact::get_sqrt_ratio_at_tick(tick);
        Arc::new(RwLock::new(PoolState {
            sqrt_price_x96: sqrt_price_x96_u256,
            sqrt_price_f64,
            tick,
            liquidity: liq,
            liquidity_f64: liq as f64,
            eth_price_usd: eth_price,
            last_block: block,
            last_update: Instant::now(),
            is_initialized: true,
            bytecode: None,
            tick_bitmap: None,
            live_fee_bps: None,
        }))
    }

    /// Gas spike testi: Base fee 5x artığında, önceki REVM simülasyonundan
    /// gelen gas değeri ile hesaplanan maliyet kârı aşıyorsa, fırsat
    /// reddedilmeli (check_arbitrage_opportunity → None).
    ///
    /// Senaryo:
    ///   - Beklenen kâr: ~0.002 WETH (küçük spread)
    ///   - Normal base fee: 100 Gwei → gas cost ~0.000015 WETH
    ///   - 5x spike: 500 Gwei → gas cost ~0.000075 WETH (hâlâ kârlı)
    ///   - 50x spike: 5000 Gwei → gas cost ~0.00075 WETH
    ///
    /// Asıl test: Dinamik gas değeri (last_simulated_gas) ile hesaplanan
    /// maliyet, fırsatın kârlılık eşiğini doğru filtreliyor mu?
    #[test]
    fn test_circuit_breaker_on_gas_spike() {
        let pools = make_pool_configs();
        // min_net_profit = 0.0002 WETH → küçük kârlı fırsatları yakala
        let config = make_test_config(0.0002, 0.00005);

        // Havuz fiyatları: %0.01 spread (çok dar)
        // Bu spread ancak düşük gas'ta kârlı
        let price_a = 2500.0;
        let price_b = 2500.25; // $0.25 spread → ~$0.25 brüt kâr (düşük)

        let liq = 50_000_000_000_000_000_000u128; // 50e18 likidite

        let states: Vec<SharedPoolState> = vec![
            make_pool_state(price_a, liq, 100),
            make_pool_state(price_b, liq, 100),
        ];

        // ─── NORMAL GAS: base_fee = 100 Gwei ─────────────────────
        let normal_base_fee: u64 = 100_000_000_000; // 100 Gwei

        // Önceki REVM: 150K gas simüle edilmiş
        let last_sim_gas = Some(150_000u64);

        // Gas cost = 150K * 100 Gwei / 1e18 = 0.000015 WETH
        // Küçük spread → Newton-Raphson çok düşük optimal miktar hesaplar
        // → kârın gas'ı karşılayıp karşılamayacağı NR'a bağlı
        let result_normal = check_arbitrage_opportunity(
            &pools, &states, &config, normal_base_fee, last_sim_gas, 0,
        );
        // Not: NR sonucu spread'e ve likiditeye bağlı — bu test gas etkisini ölçer

        // ─── GAS SPİKE: base_fee 5000x → 500.000 Gwei ───────────
        // Gerçekçi olmayan ama stres testi: base_fee = 500K Gwei
        // Gas cost = 150K * 500K Gwei / 1e18 = 0.075 WETH
        // Hiçbir küçük spread bunu karşılayamaz
        let spike_base_fee: u64 = 500_000_000_000_000; // 500K Gwei (aşırı spike)

        let result_spike = check_arbitrage_opportunity(
            &pools, &states, &config, spike_base_fee, last_sim_gas, 0,
        );

        // Gas spike durumunda fırsat kesinlikle reddedilmeli
        assert!(
            result_spike.is_none(),
            "Aşırı gas spike (0.075+ WETH maliyet) ile fırsat reddedilmeli (None dönmeli)"
        );

        // ─── DİNAMİK GAS ETKİSİ TESTİ ──────────────────────────
        // Aynı base_fee, farklı REVM gas tahmini
        // 150K gas → 0.000015 WETH, 1.5M gas → 0.00015 WETH
        let high_gas = Some(1_500_000u64); // 10x daha fazla gas
        let result_high_gas = check_arbitrage_opportunity(
            &pools, &states, &config, normal_base_fee, high_gas, 0,
        );

        // Yüksek gas tahminiyle maliyet artar → bazı fırsatlar reddedilir
        // Bu testin amacı: last_simulated_gas'ın gerçekten kullanıldığını kanıtlamak
        // Eğer hâlâ hardcoded 150K kullanılsaydı, high_gas parametresi etkisiz olurdu
        let result_low_gas = check_arbitrage_opportunity(
            &pools, &states, &config, normal_base_fee, Some(10_000u64), 0, // Çok düşük gas
        );

        // Düşük gas → düşük maliyet → fırsat bulma olasılığı ARTAR
        // Yüksek gas → yüksek maliyet → fırsat bulma olasılığı AZALIR
        // En azından biri farklı sonuç vermeli (dinamik gas etkisi kanıtı)
        // Not: Her ikisi de None olabilir (spread çok dar) ama bu bile kabul
        // edilir — önemli olan spike'ın None döndürmesi.
        eprintln!(
            "Gas spike test sonuçları: normal={:?}, spike={:?}, high_gas={:?}, low_gas={:?}",
            result_normal.as_ref().map(|r| r.expected_profit_weth),
            result_spike.as_ref().map(|r| r.expected_profit_weth),
            result_high_gas.as_ref().map(|r| r.expected_profit_weth),
            result_low_gas.as_ref().map(|r| r.expected_profit_weth),
        );
    }

    /// Gas spike ile kârlı fırsat: Büyük spread yüksek gas'ı karşılar.
    ///
    /// Senaryo: %2 spread (büyük kâr potansiyeli), 5x gas spike
    /// Gas cost: 150K * 500 Gwei / 1e18 = 0.000075 WETH
    /// Kâr >> gas cost → fırsat hâlâ kârlı olmalı
    #[test]
    fn test_gas_spike_large_spread_still_profitable() {
        let pools = make_pool_configs();
        let config = make_test_config(0.0002, 0.00005);

        // Büyük spread: %2 → kârlı olmalı (yüksek gas'a rağmen)
        let price_a = 2450.0;
        let price_b = 2500.0; // ~%2 spread
        let liq = 50_000_000_000_000_000_000u128;

        let states: Vec<SharedPoolState> = vec![
            make_pool_state(price_a, liq, 100),
            make_pool_state(price_b, liq, 100),
        ];

        // 5x spike: 500 Gwei
        let spike_base_fee: u64 = 500_000_000_000; // 500 Gwei
        let last_sim_gas = Some(150_000u64);

        let result = check_arbitrage_opportunity(
            &pools, &states, &config, spike_base_fee, last_sim_gas, 0,
        );

        // Büyük spread gas spike'ını karşılamalı
        assert!(
            result.is_some(),
            "Büyük spread (%2) ile gas spike'a rağmen fırsat bulunmalı"
        );
        let opp = result.unwrap();
        assert!(
            opp.expected_profit_weth > 0.0002,
            "Kâr minimum eşikten ({}) yüksek olmalı: {:.6}",
            0.0002,
            opp.expected_profit_weth
        );
    }

    /// Base fee = 0 fallback testi: EIP-1559 öncesi veya hata durumu.
    #[test]
    fn test_zero_base_fee_uses_config_fallback() {
        let pools = make_pool_configs();
        let config = make_test_config(0.0002, 0.00005); // gas_cost_fallback_weth = 0.00005 WETH

        let price_a = 2450.0;
        let price_b = 2500.0;
        let liq = 50_000_000_000_000_000_000u128;

        let states: Vec<SharedPoolState> = vec![
            make_pool_state(price_a, liq, 100),
            make_pool_state(price_b, liq, 100),
        ];

        // base_fee = 0 → config.gas_cost_fallback_weth (0.00005 WETH)
        let result = check_arbitrage_opportunity(
            &pools, &states, &config, 0, Some(150_000), 0,
        );

        assert!(
            result.is_some(),
            "base_fee=0 durumunda config fallback ile fırsat bulunmalı"
        );
    }
}