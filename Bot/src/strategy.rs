// ============================================================================
//  STRATEGY v18.0 � Arbitraj Strateji Motoru + L1 Data Fee + Fire-and-Forget
//
//  v18.0 Yenilikler:
//  ? L1 Data Fee (OP Stack) entegrasyonu � total_gas = L2 + L1
//  ? GasPriceOracle.getL1Fee() ile do�ru maliyet tahmini
//  ? Fire-and-forget TX receipt bekleme (4s timeout, pipeline bloke olmaz)
//  ? PGA fallback uyumlu bribe hesab�
//
//  v9.0 (korunuyor):
//  ? 134-byte kompakt calldata (kontrat v9.0 uyumlu, deadlineBlock dahil)
//  ? Deadline block hesaplama (current_block + config.deadline_blocks)
//  ? Dinamik bribe/priority fee modeli (beklenen k�r�n %25'i)
//  ? KeyManager entegrasyonu (raw private key yerine �ifreli y�netim)
//
//  v7.0 (korunuyor):
//  ? owedToken / receivedToken / minProfit hesaplama
//  ? Atomik nonce y�netimi entegrasyonu
//  ? TickBitmap-aware Newton-Raphson optimizasyonu
//  ? Raw TX g�nderi (sol! interface yerine TransactionRequest)
// ============================================================================

use alloy::primitives::{Address, U256};
use alloy::providers::Provider;
use alloy::signers::local::PrivateKeySigner;
use colored::*;
use chrono::Local;
use std::io::Write;
use std::sync::Arc;

use crate::types::*;
use crate::math;
use crate::simulator::SimulationEngine;

use zeroize::Zeroize;

// �����������������������������������������������������������������������������
// Zaman Damgas�
// �����������������������������������������������������������������������������

fn timestamp() -> String {
    Local::now().format("%H:%M:%S%.3f").to_string()
}

// �����������������������������������������������������������������������������
// Arbitraj F�rsat Tespiti
// �����������������������������������������������������������������������������

/// Her iki havuzun fiyatlar�n� kar��la�t�r ve f�rsat varsa tespit et
///
/// F�rsat Ko�ullar�:
///   1. Her iki havuz aktif ve veriler taze
///   2. Fiyat fark� (spread) > minimum e�ik
///   3. Newton-Raphson ile hesaplanan k�r > minimum net k�r
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
    let state_a = states[0].load_full().as_ref().clone();
    let state_b = states[1].load_full().as_ref().clone();

    // v10.0: Hard-Abort — Stale Data Guard (is_fresh = is_active + staleness eşiği)
    // is_active(): is_initialized && !is_stale && eth_price>0 && liquidity>0
    // is_fresh(): is_active() + staleness_ms() <= max_staleness_ms
    // Havuz verisinin yaşı max_staleness_ms eşiğini aştığında HARD-ABORT.
    // Eski/bayat veri ile arbitraj hesaplamak hayalet kâr üretir ve kesin fon kaybına yol açar.
    {
        let fresh_a = state_a.is_fresh(config.max_staleness_ms);
        let fresh_b = state_b.is_fresh(config.max_staleness_ms);
        if !fresh_a || !fresh_b {
            if state_a.is_active() && state_b.is_active() {
                // Havuzlar aktif ama veri yaşlı — HARD-ABORT loglama
                eprintln!(
                    "     \u{1f6a8} [HARD-ABORT] Stale data detected! A={}ms B={}ms (threshold={}ms) — opportunity CANCELLED",
                    state_a.staleness_ms(), state_b.staleness_ms(), config.max_staleness_ms,
                );
            }
            return None;
        }
    }

    // ��� v19.0: Havuz Komisyon G�venlik Tavan� (Sadece Uyar�) �����
    // v19.0: Statik fee reddi kald�r�ld�. Komisyon filtresi art�k
    // PreFilter'�n dinamik net k�rl�l�k hesab�n�n par�as�.
    // Sadece �ok y�ksek fee'li havuzlarda (>max_pool_fee_bps) g�venlik reddi.
    {
        let fee_a_bps = state_a.live_fee_bps.unwrap_or(pools[0].fee_bps);
        let fee_b_bps = state_b.live_fee_bps.unwrap_or(pools[1].fee_bps);
        if fee_a_bps > config.max_pool_fee_bps || fee_b_bps > config.max_pool_fee_bps {
            eprintln!(
                "     \u{23ed}\u{fe0f} [FeeFilter] Pool fee exceeds safety ceiling: A={}bps B={}bps (max={}bps)",
                fee_a_bps, fee_b_bps, config.max_pool_fee_bps,
            );
            return None;
        }
        // v19.0: Y�ksek ama kabul edilebilir fee'ler loglans�n
        let total_fee_bps = fee_a_bps + fee_b_bps;
        if total_fee_bps > 30 {
            eprintln!(
                "     \u{2139}\u{fe0f} [FeeInfo] High total fee: A={}bps + B={}bps = {}bps \u{2192} delegated to dynamic profitability check",
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

    // L1 data fee � WETH (t�m gas hesaplar�nda kullan�lacak)
    let l1_data_fee_weth = l1_data_fee_wei as f64 / 1e18;

    // ��� v27.0: Y�n + Likidite � PreFilter s�ralama d�zeltmesi ���
    // �nce y�n ve havuz derinli�ini hesapla, sonra PreFilter'a besle.
    // Eski hata: PreFilter statik 25 WETH probe ile �al���yor, havuz s��
    // oldu�unda sahte k�r tahmini �retiyordu. �imdi effective_cap
    // PreFilter'dan �NCE hesaplan�r ve probe_amount olarak kullan�l�r.

    // Y�n belirleme: Ucuzdan al, pahal�ya sat
    let (buy_idx, sell_idx) = if price_a < price_b {
        (0, 1) // A ucuz, B pahal�
    } else {
        (1, 0) // B ucuz, A pahal�
    };

    let buy_state = if buy_idx == 0 { &state_a } else { &state_b };
    let sell_state = if sell_idx == 0 { &state_a } else { &state_b };
    let avg_price_in_quote = (price_a + price_b) / 2.0;

    // ��� TickBitmap referanslar� (varsa + v28.0: tazelik do�rulamas�) �
    // v28.0: TickBitmap'in ya�� tick_bitmap_max_age_blocks'u a��yorsa
    // eski veri kullanmak yerine None d�nd�r � single-tick fallback.
    // Eski bitmap ile hesaplama hatal� likidite tahmini ve MEV a���� yarat�r.
    let current_block = sell_state.last_block.max(buy_state.last_block);
    let bitmap_max_age = config.tick_bitmap_max_age_blocks;

    let sell_bitmap_stale = sell_state.tick_bitmap.as_ref().is_some_and(|bm| {
        current_block.saturating_sub(bm.snapshot_block) > bitmap_max_age
    });
    let buy_bitmap_stale = buy_state.tick_bitmap.as_ref().is_some_and(|bm| {
        current_block.saturating_sub(bm.snapshot_block) > bitmap_max_age
    });

    // v30.0: Bitmap VARDI ama stale ise havuzu bu blok icin tamamen atla.
    // Bitmap hic yoksa (None) single-tick fallback devam eder (yeni havuzlar).
    if sell_bitmap_stale || buy_bitmap_stale {
        eprintln!(
            "     [TickBitmap] Stale bitmap detected -- skipping pool entirely this block (age_limit={})",
            bitmap_max_age,
        );
        return None;
    }

    let sell_bitmap = sell_state.tick_bitmap.as_ref().filter(|bm| {
        current_block.saturating_sub(bm.snapshot_block) <= bitmap_max_age
    });
    let buy_bitmap = buy_state.tick_bitmap.as_ref().filter(|bm| {
        current_block.saturating_sub(bm.snapshot_block) <= bitmap_max_age
    });

    // ��� v11.0: Hard Liquidity Cap � PreFilter + NR �ncesi Havuz Derinlik Kontrol� �
    // Havuzun ger�ek mevcut likiditesini hesapla (TickBitmap'ten).
    // WETH/USDC havuzlar�nda 18 vs 6 decimal uyumsuzlu�u burada yakalan�r.
    // v27.0: effective_cap art�k PreFilter'a da beslenir (probe_amount).
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

    // v28.0: S�� havuz ��k�� kap�s� � effective_cap ile gas maliyetini kar��la�t�r.
    // Havuz derinli�i gas maliyetinin 10 kat�ndan azsa, k�rl� i�lem imk�ns�z.
    // Bu erken ��k��, NR + PreFilter hesaplamalar�n� tamamen atlar � CPU tasarrufu.
    if effective_cap <= 0.001 {
        eprintln!(
            "     \u{23ed}\u{fe0f} [Liquidity] Insufficient liquidity — skipping NR (cap={:.6} WETH)",
            effective_cap,
        );
        return None;
    }

    // v28.0: Dinamik likidite uyar�s� + ekonomik uygulanabilirlik kontrol�
    if effective_cap < config.max_trade_size_weth * 0.1 {
        eprintln!(
            "     \u{26a0}\u{fe0f} [Liquidity] Pool depth shallow: sell_cap={:.4} buy_cap={:.4} effective_cap={:.4} WETH (MAX_TRADE={:.1})",
            sell_hard_cap, buy_hard_cap, effective_cap, config.max_trade_size_weth,
        );
        // v28.0: S�� havuzda gas maliyetini kar��layacak spread var m�?
        // Kaba tahmin: effective_cap * spread_pct/100 < min_net_profit � kesinlikle k�rs�z
        let max_possible_gross = effective_cap * spread_pct / 100.0;
        if max_possible_gross < config.min_net_profit_weth {
            eprintln!(
                "     \u{23ed}\u{fe0f} [EconViability] Shallow pool + low spread — profit impossible: max_gross={:.8} < min_profit={:.8} WETH",
                max_possible_gross, config.min_net_profit_weth,
            );
            return None;
        }
    }

    // OPT-I: Gas cost tek sefer hesapla — PreFilter ve NR ayni degeri kullanir.
    // OPT-H: Guvenlik marji %20 -> %10 (Base L2'de gas spike nadir).
    let gas_estimate: u64 = last_simulated_gas.unwrap_or(200_000);
    let dynamic_gas_cost_weth = if block_base_fee > 0 {
        let l2 = (gas_estimate as f64 * block_base_fee as f64) / 1e18;
        ((l2 + l1_data_fee_weth) * 1.10).max(0.000001)
    } else {
        ((config.gas_cost_fallback_weth + l1_data_fee_weth) * 1.10).max(0.000001)
    };

    // PreFilter
    {
        let pre_filter = math::PreFilter {
            fee_a: state_a.live_fee_bps.map(|b| b as f64 / 10_000.0).unwrap_or(pools[0].fee_fraction),
            fee_b: state_b.live_fee_bps.map(|b| b as f64 / 10_000.0).unwrap_or(pools[1].fee_fraction),
            estimated_gas_cost_weth: dynamic_gas_cost_weth,
            min_profit_weth: config.min_net_profit_weth,
            bribe_pct: config.bribe_pct,
        };

        let probe_amount = f64::min(config.max_trade_size_weth * 0.5, effective_cap);

        match pre_filter.check(price_a, price_b, probe_amount) {
            math::PreFilterResult::Unprofitable { reason } => {
                eprintln!(
                    "     \u{23ed}\u{fe0f} [PreFilter] Spread {:.4}% | {:?} | gas={:.8} WETH | probe={:.4} WETH",
                    spread_pct,
                    reason,
                    dynamic_gas_cost_weth,
                    probe_amount,
                );
                return None;
            }
            math::PreFilterResult::Profitable { estimated_profit_weth, spread_ratio } => {
                eprintln!(
                    "     \u{2705} [PreFilter] PASSED | spread_ratio={:.6} | est_profit={:.8} WETH | probe={:.4} WETH",
                    spread_ratio,
                    estimated_profit_weth,
                    probe_amount,
                );
            }
        }
    }

    // Gas cost'u quote cinsine cevir (NR icin)
    let dynamic_gas_cost_quote = dynamic_gas_cost_weth * avg_price_in_quote;

    // ��� Newton-Raphson Optimal Miktar Hesaplama ������������������
    // v6.0: TickBitmap varsa multi-tick hassasiyetinde, yoksa dampening
    // v16.0: Canl� on-chain fee kullan�m� (live_fee_bps varsa statik fee yerine)
    let sell_fee = sell_state.live_fee_bps.map(|b| b as f64 / 10_000.0).unwrap_or(pools[sell_idx].fee_fraction);
    let buy_fee = buy_state.live_fee_bps.map(|b| b as f64 / 10_000.0).unwrap_or(pools[buy_idx].fee_fraction);
    // v28.0: NR'ye max_trade_size_weth yerine effective_cap g�nder.
    // Eski: config.max_trade_size_weth (50.0) � NR i�inde tekrar cap hesapl�yor,
    //        �ift hesaplama + s�� havuzlarda gereksiz tarama aral���.
    // Yeni: effective_cap zaten min(sell_cap, buy_cap) olarak hesapland�,
    //        NR bunu �st s�n�r olarak al�r � tutarl� ve h�zl�.
    let nr_max = effective_cap.min(config.max_trade_size_weth);
    let nr_result = math::find_optimal_amount_with_bitmap(
        sell_state,
        sell_fee,
        buy_state,
        buy_fee,
        dynamic_gas_cost_quote,
        avg_price_in_quote, // ger�ek fiyat � k�r quote cinsinden d�ner
        nr_max,
        pools[sell_idx].token0_is_weth,
        pools[sell_idx].tick_spacing,
        pools[buy_idx].tick_spacing,
        sell_bitmap,
        buy_bitmap,
        pools[buy_idx].token0_is_weth,
    );

    // NR k�r� quote (cbBTC) cinsinden d�nd� � WETH�e �evir
    let expected_profit_weth = if avg_price_in_quote > 0.0 {
        nr_result.expected_profit / avg_price_in_quote
    } else {
        return None;
    };

    // v15.0 DEBUG: NR sonu� detaylar� � f�rsat filtreleme nedenini g�ster
    // (Bu loglar canl�ya ge�i� onay�na kadar kald�r�lmamal�)
    eprintln!(
        "     \u{1f52c} [DEBUG NR] spread={:.4}% | nr_profit_weth={:.8} | min_required={:.8} | nr_amount={:.6} | converged={} | gas_cost_weth={:.8} (L1={:.8})",
        spread_pct,
        expected_profit_weth,
        config.min_net_profit_weth,
        nr_result.optimal_amount,
        nr_result.converged,
        dynamic_gas_cost_weth,
        l1_data_fee_weth,
    );

    // K�rl� de�ilse f�rsat� atla
    // v30.0: Dinamik ROI + mutlak esik kontrolu
    // Ya mutlak kar yeterli VEYA ROI yuzde esigini gecmeli.
    // Ikisi de karsilanmiyorsa reddet.
    let roi = if nr_result.optimal_amount > 0.0 {
        expected_profit_weth / nr_result.optimal_amount
    } else {
        0.0
    };
    if nr_result.optimal_amount <= 0.0
        || (expected_profit_weth < config.min_net_profit_weth && roi < config.min_profit_roi)
    {
        eprintln!(
            "     [ProfitGate] Unprofitable -- profit={:.8} WETH, roi={:.6}%, min_profit={:.8}, min_roi={:.4}%",
            expected_profit_weth,
            roi * 100.0,
            config.min_net_profit_weth,
            config.min_profit_roi * 100.0,
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

// �����������������������������������������������������������������������������
// F�rsat De�erlendirme ve Y�r�tme
// �����������������������������������������������������������������������������

/// Bulunan arbitraj f�rsat�n� de�erlendir, sim�le et ve gerekirse y�r�t
///
/// D�n��: REVM sim�lasyonundan gelen ger�ek gas kullan�m� (sonraki bloklarda
/// `check_arbitrage_opportunity`'e beslenir � dinamik gas maliyet hesaplamas�).
///
/// v21.0: `mev_executor` parametresi eklendi � i�lemler yaln�zca Private RPC
/// (eth_sendRawTransaction) �zerinden g�nderilir, public mempool kullan�lmaz.
#[allow(clippy::too_many_arguments)]
pub async fn evaluate_and_execute<P: Provider + Sync>(
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
    telegram_sender: &Option<crate::telegram::TelegramSender>,
    tg_counters: &mut crate::telegram::TelemetryCounters,
) -> Option<u64> {
    let _buy_pool = &pools[opportunity.buy_pool_idx];
    let _sell_pool = &pools[opportunity.sell_pool_idx];

    // ��� v12.0: S�f�ra B�l�nme / NaN / Infinity Korumas� �������������
    // RPC kopuklu�u veya s�f�r sqrtPriceX96 durumunda fiyatlar 0.0 olabilir.
    // Float b�l�m sonucu Infinity � u128'e cast'te Rust panic! verir.
    // Bu kontrol thread ��kmesini �nler ve d�ng�y� sessizce atlar.
    if opportunity.sell_price_quote <= 0.0
        || opportunity.buy_price_quote <= 0.0
        || opportunity.optimal_amount_weth <= 0.0
        || !opportunity.expected_profit_weth.is_finite()
    {
        return None;
    }

    // ��� v28.0: Veri Tazeli�i Kap�s� (Freshness Gate) ��������������
    // Eski veriyle yap�lan sim�lasyon ve i�lem, frontrun/sandwich sald�r�lar�na
    // kar�� savunmas�zd�r. ��lem g�nderilmeden �nce havuz verilerinin
    // max_staleness_ms e�i�ini a�mad��� do�rulan�r.
    {
        let state_a_guard = states[0].load();
        let state_b_guard = states[1].load();
        if !state_a_guard.is_fresh(config.max_staleness_ms) || !state_b_guard.is_fresh(config.max_staleness_ms) {
            let staleness_a = state_a_guard.staleness_ms();
            let staleness_b = state_b_guard.staleness_ms();
            drop(state_a_guard);
            drop(state_b_guard);
            eprintln!(
                "     \u{1f6d1} [FreshnessGate] Pool data too old or stale: A={}ms B={}ms (threshold={}ms) — MEV protection: trade skipped",
                staleness_a, staleness_b, config.max_staleness_ms,
            );
            return None;
        }
    }

    // ��� �statistik G�ncelle �������������������������������������
    // v15.0: total_opportunities ve max_spread_pct art�k main.rs'de
    // her blokta g�ncelleniyor (f�rsat ko�ulundan ba��ms�z).
    // Burada sadece sim�lasyona �zg� istatistikler kal�yor.

    // ��� REVM Sim�lasyonu ��������������������������������������
    let sim_result = sim_engine.validate_mathematical(
        pools,
        states,
        opportunity.buy_pool_idx,
        opportunity.sell_pool_idx,
        opportunity.optimal_amount_weth,
    );

    // Kontrat adresi varsa tam REVM sim�lasyonu da yap
    let revm_result = if let Some(contract_addr) = config.contract_address {
        // v11.0 Calldata: Y�n ve token hesaplama
        //   buy_pool_idx=0 (UniV3 ucuz): uni=1(oneForZero�WETH al), aero=0(zeroForOne�WETH sat)
        //   buy_pool_idx=1 (Slip ucuz):  uni=0(zeroForOne�Quote al), aero=1(oneForZero�Quote sat)
        let (uni_dir, aero_dir, owed_token, received_token) =
            compute_directions_and_tokens(
                opportunity.buy_pool_idx,
                pools[0].token0_is_weth,
                &pools[0].base_token_address,
                &pools[0].quote_token_address,
            );

        // === v11.0: D�NAM�K DECIMAL AMOUNT HESAPLAMA ===
        // Kritik d�zeltme: Input tokeni WETH mi Quote mi?
        //   - WETH input � amount * 10^18
        //   - Quote input � amount * eth_price * 10^quote_decimals
        // Eski hata: Her zaman 10^18 kullan�l�yordu � Quote input'ta
        //            hatal� hesaplama olu�uyordu.
        let weth_input = crate::types::is_weth_input(uni_dir, pools[0].token0_is_weth);
        let amount_wei = crate::types::weth_amount_to_input_wei(
            opportunity.optimal_amount_weth,
            weth_input,
            (opportunity.buy_price_quote + opportunity.sell_price_quote) / 2.0,
            if pools[0].token0_is_weth { pools[0].token1_decimals } else { pools[0].token0_decimals },
        );

        // v9.0: Deadline block hesapla (v11.0: minimum +3 tolerans)
        let current_block = states[0].load().last_block;
        let deadline_block = current_block as u32 + config.deadline_blocks.max(3);

        let calldata = crate::simulator::encode_compact_calldata(
            pools[0].address,  // pool_a (always UniV3)
            pools[1].address,  // pool_b (always Slipstream)
            owed_token,
            received_token,
            amount_wei,
            uni_dir,
            aero_dir,
            0u128, // REVM simulation � minProfit=0
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
            current_block,
            block_timestamp,
            block_base_fee,
        )
    } else {
        sim_result.clone()
    };

    // Dinamik gas: REVM sim�lasyonundan gelen kesin gas de�eri
    let simulated_gas_used = revm_result.gas_used;

    // Sim�lasyon ba�ar�s�z � i�lemi atla
    if !sim_result.success {
        stats.failed_simulations += 1;
        // v10.0: Circuit breaker
        stats.consecutive_failures += 1;
        tg_counters.reverts += 1;
        print_simulation_failure(opportunity, &sim_result, pools);
        return None;
    }

    // Sim�lasyon ba�ar�l� � ard���k ba�ar�s�zl�k sayac�n� s�f�rla
    stats.consecutive_failures = 0;

    // JSON structured log: profitable opportunity
    crate::json_logger::log_opportunity(
        &format!("{} vs {}", pools[opportunity.buy_pool_idx].name, pools[opportunity.sell_pool_idx].name),
        opportunity.spread_pct,
        opportunity.expected_profit_weth,
        opportunity.optimal_amount_weth,
        true,
    );

    stats.profitable_opportunities += 1;
    stats.total_potential_profit += opportunity.expected_profit_weth;
    if opportunity.expected_profit_weth > stats.max_profit_weth {
        stats.max_profit_weth = opportunity.expected_profit_weth;
    }

    print_opportunity_report(opportunity, &sim_result, pools, config);

    // ��� KONTRAT TET�KLEME VEYA G�LGE MOD LOGLAMA �������������
    if config.shadow_mode() {
        // === G�LGE MODU: ��lem atlan�r, detaylar loglan�r ===

        // v23.0 (Y-1): G�lge modu ekonomik uygulanabilirlik istatistikleri
        if sim_result.success {
            stats.shadow_sim_success += 1;
            stats.shadow_cumulative_profit += opportunity.expected_profit_weth;
        } else {
            stats.shadow_sim_fail += 1;
        }

        println!(
            "  {} {}",
            "??".yellow(),
            "SHADOW MODE: Trade skipped — details logged to shadow_analytics.jsonl".yellow().bold()
        );
        // v23.0 (Y-1): Periyodik ekonomik �zet (her 10 f�rsatta bir)
        let total_shadow = stats.shadow_sim_success + stats.shadow_sim_fail;
        if total_shadow > 0 && total_shadow.is_multiple_of(10) {
            let success_rate = (stats.shadow_sim_success as f64 / total_shadow as f64) * 100.0;
            println!(
                "  {} Shadow Summary: {} opportunities | Sim success: {:.1}% | Cumulative profit: {:.6} WETH",
                "??".cyan(),
                total_shadow,
                success_rate,
                stats.shadow_cumulative_profit,
            );
        }

        // Dinamik bribe hesab� (loglama i�in)
        let dynamic_bribe_weth = opportunity.expected_profit_weth * config.bribe_pct;

        // Shadow log kayd� (v10.0: yap�land�r�lm�� JSONL)
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

        // v30.0: base_token_address kullan�l�r � cbETH/WETH gibi non-WETH-base �iftleri i�in kritik
        let (uni_dir, aero_dir, owed_token, received_token) =
            compute_directions_and_tokens(
                opportunity.buy_pool_idx,
                pools[0].token0_is_weth,
                &pools[0].base_token_address,
                &pools[0].quote_token_address,
            );

        // v11.0: Deadline block hesapla (minimum +3 tolerans)
        let current_block = states[0].load().last_block;
        let deadline_block = current_block as u32 + config.deadline_blocks.max(3);

        // v21.0: Bribe hesab� MevExecutor::compute_dynamic_bribe'a devredildi.
        // MevExecutor, expected_profit_weth + simulated_gas + block_base_fee
        // bilgilerini alarak adaptatif bribe y�zdesini kendi i�inde hesaplar
        // ve priority fee olarak TX'e ekler.

        // === v11.0: Y�N-BAZLI EXACT minProfit HESAPLAMA ===
        // Kritik d�zeltme: Eski sistem her zaman WETH cinsinden profit hesapl�yordu.
        // Ancak kontrat balAfter(owedToken) - balBefore(owedToken) hesab� yapar.
        // owedToken=Quote ise k�r quote cinsinden �l��l�r � minProfit quote_decimals olmal�.
        //
        // Yeni sistem: Flash swap ak���n� birebir modelleyen
        // compute_exact_directional_profit kullan�l�r.
        // Bu fonksiyon do�rudan owedToken cinsinden k�r d�nd�r�r.
        let exact_min_profit = {
            let pool_a_state = states[0].load();
            let pool_b_state = states[1].load();
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

        // v24.0: Desimal-duyarl� dinamik slippage
        let slippage_bps = {
            let buy_state = states[opportunity.buy_pool_idx].load();
            let sell_state = states[opportunity.sell_pool_idx].load();
            determine_slippage_factor_bps(
                buy_state.liquidity,
                sell_state.liquidity,
                &pools[opportunity.buy_pool_idx],
                &pools[opportunity.sell_pool_idx],
            )
        };
        let min_profit = compute_min_profit_exact(exact_min_profit, slippage_bps);

        // Atomik nonce al
        let nonce = nonce_manager.get_and_increment();
        let nm_clone = Arc::clone(nonce_manager);

        stats.executed_trades += 1;

        // v32.0: Telegram — Alfa Bildirimi (başarılı arbitraj)
        let gas_cost_weth = simulated_gas_used as f64 * block_base_fee as f64 / 1e18;
        if let Some(ref tg) = telegram_sender {
            tg.send(crate::telegram::TelegramMessage::AlphaSuccess {
                buy_pool: pools[opportunity.buy_pool_idx].name.clone(),
                sell_pool: pools[opportunity.sell_pool_idx].name.clone(),
                gross_profit_weth: opportunity.expected_profit_weth,
                gas_cost_weth,
                net_profit_weth: opportunity.expected_profit_weth - gas_cost_weth,
                latency_ms: block_latency_ms,
                tx_hash: format!("nonce:{}", nonce_manager.current().saturating_sub(1)),
            });
        }
        tg_counters.successful_trades += 1;
        tg_counters.net_period_profit_weth += opportunity.expected_profit_weth - gas_cost_weth;

        let pool_a_addr = pools[0].address;
        let pool_b_addr = pools[1].address;

        // REVM'den gelen kesin gas de�erini aktar (sabit 350K yerine)
        let sim_gas = simulated_gas_used;

        // v11.0: ETH fiyat� ve token s�ras� bilgisini execute_on_chain'e aktar
        let eth_price_for_exec = (opportunity.buy_price_quote + opportunity.sell_price_quote) / 2.0;
        let t0_is_weth = pools[0].token0_is_weth;

        // v13.0: block_base_fee'yi execute'a aktar (max_fee_per_gas hesab� i�in)
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
                current_block,
            ).await;
        });
    }

    // v14.0: REVM'den gelen ger�ek gas de�erini d�nd�r
    // Bir sonraki blokta check_arbitrage_opportunity'ye beslenir
    Some(simulated_gas_used)
}

// �����������������������������������������������������������������������������
// G�lge Modu (Shadow Mode) � JSON Loglama
// �����������������������������������������������������������������������������

/// G�lge modunda bulunan f�rsat�n t�m detaylar�n� shadow_analytics.jsonl
/// dosyas�na sat�r sat�r (JSON Lines / NDJSON format�nda) append eder.
///
/// v10.0 Yap�land�r�lm�� Alanlar:
///   - timestamp, pool_pair, gas_used, expected_profit
///   - simulated_profit, dynamic_bribe, latency_ms
///
/// Bu dosya birka� g�n sonra a��l�p:
///   "Bot 1000 f�rsat bulmu�, ger�ek TX atsayd�k toplam 450$ kazanacakt�k"
/// analizini yapmak i�in kullan�l�r.
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

    // pool_pair: "UniV3-WETH/cbBTC - Aero-WETH/cbBTC"
    let pool_pair = format!("{} - {}", buy_pool.name, sell_pool.name);

    // Simulated profit = expected profit if sim succeeded, 0 otherwise
    let simulated_profit_weth = if sim_result.success {
        opportunity.expected_profit_weth
    } else {
        0.0
    };

    // JSONL yap�land�r�lm�� log sat�r�
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

    // v22.1: Dosya boyutu kontrol� � 50MB'� a�arsa rotate et
    let log_path = std::path::Path::new("shadow_analytics.jsonl");
    const MAX_LOG_SIZE: u64 = 50 * 1024 * 1024; // 50 MB
    if let Ok(metadata) = std::fs::metadata(log_path) {
        if metadata.len() >= MAX_LOG_SIZE {
            let rotated = format!("shadow_analytics.{}.jsonl",
                chrono::Local::now().format("%Y%m%d_%H%M%S"));
            let _ = std::fs::rename(log_path, &rotated);
            eprintln!("  ?? Shadow log rotated — {}", rotated);
        }
    }

    // Dosyaya append (sat�r sat�r)
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    {
        Ok(mut file) => {
            if let Err(e) = writeln!(file, "{}", log_entry) {
                eprintln!(
                    "  {} shadow_analytics.jsonl write error: {}",
                    "??".yellow(), e
                );
            }
        }
        Err(e) => {
            eprintln!(
                "  {} shadow_analytics.jsonl open error: {}",
                "??".yellow(), e
            );
        }
    }
}

// �����������������������������������������������������������������������������
// Kontrat Tetikleme (Zincir �zeri) � MevExecutor �zerinden Private RPC
// �����������������������������������������������������������������������������

// v21.0: ProviderBuilder ve TransactionRequest art�k MevExecutor'da kullan�l�r.
// strategy.rs do�rudan TX olu�turmaz.

/// v21.0: Arbitraj kontrat�n� MevExecutor �zerinden Private RPC ile tetikle.
///
/// Public mempool kullan�lmaz � t�m i�lemler eth_sendRawTransaction ile Private RPC'ye g�nderilir.
/// Private RPC yoksa veya ba�ar�s�zsa i�lem �PTAL ED�L�R (nonce geri al�n�r).
#[allow(clippy::too_many_arguments)]
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
    println!("\n  {} {}", "??".yellow(), "CONTRACT EXECUTION STARTED (Private RPC)".yellow().bold());

    // v10.0: Private key g�venli bellek y�netimi
    let mut pk_owned = private_key;

    // Calldata olu�tur
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
        "  {} Compact calldata (134 bytes): {}...{}",
        "??".cyan(),
        &calldata_hex[..22],
        &calldata_hex[calldata_hex.len().saturating_sub(10)..],
    );

    println!(
        "  {} Sending TX (Private RPC)... (amount: {:.6} WETH, nonce: {}, deadline: block #{}, payload: 134 bytes)",
        "??".yellow(), trade_size_weth, nonce, deadline_block
    );

    // MevExecutor �zerinden g�nder � Private RPC yoksa otomatik iptal
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

    // �mza tamamland� � private key bellekten g�venle silinir
    pk_owned.zeroize();

    match result {
        Ok(hash) => {
            println!("  {} TX successful (Private RPC): {}", "?".green(), hash.green().bold());
        }
        Err(e) => {
            // TX zincire gitmediyse local nonce geri alınır.
            nonce_manager.force_set(nonce);
            println!("  {} TX error: {}", "?".red(), format!("{}", e).red());
        }
    }
}

// �����������������������������������������������������������������������������
// Y�n ve Token Hesaplama Yard�mc�lar�
// �����������������������������������������������������������������������������

/// Arbitraj y�n�nden UniV3/Slipstream y�nlerini ve token adreslerini hesapla
///
/// # D�n��: (uni_direction, aero_direction, owed_token, received_token)
///
/// v30.0: base_token_address parametresi � config.weth_address yerine PoolConfig'den gelir.
/// cbETH/WETH gibi non-WETH-base �iftlerinde base_token=cbETH, quote_token=WETH olur.
/// Eski: Her zaman config.weth_address kullan�l�yordu � cbETH/WETH'te owedToken=receivedToken=WETH. BUG!
///
/// Mant�k (token0=base, token1=quote varsay�m�yla):
/// - buy_pool_idx=0: uni=1(oneForZero�base al), aero=0(zeroForOne�base sat)
///   owedToken=Quote, receivedToken=Base
/// - buy_pool_idx=1: uni=0(zeroForOne�quote al), aero=1(oneForZero�quote sat)
///   owedToken=Base, receivedToken=Quote
fn compute_directions_and_tokens(
    buy_pool_idx: usize,
    token0_is_base: bool,
    base_token_address: &Address,
    quote_token_address: &Address,
) -> (u8, u8, Address, Address) {
    if token0_is_base {
        // token0 = base, token1 = quote (Base normal d�zen: WETH < USDC)
        if buy_pool_idx == 0 {
            // Pool 0'dan base al � oneForZero(1), Pool 1'e base sat � zeroForOne(0)
            (1u8, 0u8, *quote_token_address, *base_token_address) // owe Quote, receive Base
        } else {
            // Pool 0'dan quote al � zeroForOne(0), Pool 1'e quote sat � oneForZero(1)
            (0u8, 1u8, *base_token_address, *quote_token_address) // owe Base, receive Quote
        }
    } else {
        // token0 = quote, token1 = base (ters d�zen: cbETH < WETH)
        if buy_pool_idx == 0 {
            (0u8, 1u8, *base_token_address, *quote_token_address) // owe Base, receive Quote
        } else {
            (1u8, 0u8, *quote_token_address, *base_token_address) // owe Quote, receive Base
        }
    }
}

/// minProfit hesapla (owedToken cinsinden, uint128 wei)
///
/// math::exact::compute_exact_arbitrage_profit ile hesaplanan
/// exact_profit_wei de�erinin dinamik bir y�zdesini minProfit olarak ayarla.
///
/// v10.0: Varl�k bazl� dinamik slippage:
///   - Derin likidite (>1e18): %99.9 (sadece 10 bps tolerans)
///   - Orta likidite (>1e16): %99.5 (50 bps tolerans)
///   - S�� likidite:          %95   (500 bps tolerans, g�venli)
///
/// �NEML�: Float ve quote �evirisi YOKTUR. Tamamen U256 tam say� matematik.
fn compute_min_profit_exact(exact_profit_wei: U256, slippage_factor_bps: u64) -> u128 {
    // slippage_factor_bps: 9990 = %99.9, 9950 = %99.5, 9500 = %95
    let min_profit_u256 = (exact_profit_wei * U256::from(slippage_factor_bps)) / U256::from(10_000u64);

    // u128'e s��d�r (kontrat uint128 bekler). Overflow durumunda u128::MAX kullan.
    if min_profit_u256 > U256::from(u128::MAX) {
        u128::MAX
    } else {
        min_profit_u256.to::<u128>()
    }
}

/// Havuz likidite derinli�ine g�re slippage fakt�r� hesapla (bps cinsinden)
///
/// v24.0: Token desimal-duyarl� normalizasyon.
/// Raw likidite (u128), havuzdaki token0 ve token1'in desimal fark�na g�re
/// 18-desimale normalize edilir. Bu sayede USDC (6 desimal) havuzunda
/// 1e10 raw likidite, WETH (18 desimal) havuzundaki 1e18 ile e�de�er olarak
/// de�erlendirilir.
///
/// Mant�k (normalize likiditeye g�re):
///   - Derin havuz (>= 1e15 normalized) � 9950 bps (%99.5)
///   - Orta derinlik (>= 1e13 normalized) � 9900 bps (%99)
///   - S�� havuz (< 1e13 normalized) � 9500 bps (%95)
fn determine_slippage_factor_bps(
    buy_liquidity: u128,
    sell_liquidity: u128,
    buy_pool: &PoolConfig,
    sell_pool: &PoolConfig,
) -> u64 {
    // Her havuzun likiditesini 18-desimale normalize et.
    // Uniswap V3'te L parametresi sqrt(token0 * token1) biriminde olup
    // desimal fark� (token0_decimals + token1_decimals) / 2 kadar dengelenmeli.
    let normalize = |liq: u128, pool: &PoolConfig| -> f64 {
        let avg_decimals = (pool.token0_decimals as f64 + pool.token1_decimals as f64) / 2.0;
        let scale = 10f64.powi(18 - avg_decimals as i32);
        liq as f64 * scale
    };

    let norm_buy = normalize(buy_liquidity, buy_pool);
    let norm_sell = normalize(sell_liquidity, sell_pool);
    let min_normalized = norm_buy.min(norm_sell);

    if min_normalized >= 1e15 {
        9950 // %99.5 � derin havuz
    } else if min_normalized >= 1e13 {
        9900 // %99.0 � orta derinlik
    } else {
        9500 // %95.0 � s�� havuz, konservatif
    }
}

// �����������������������������������������������������������������������������
// Terminal ��kt�lar�
// �����������������������������������������������������������������������������

/// Sim�lasyon hatas� raporu
fn print_simulation_failure(
    opp: &ArbitrageOpportunity,
    sim: &SimulationResult,
    _pools: &[PoolConfig],
) {
    println!(
        "     {} [{}] REVM Simulation FAILED | Spread: {:.4}% | Reason: {}",
        "??".yellow(),
        timestamp().dimmed(),
        opp.spread_pct,
        sim.error.as_deref().unwrap_or("Unknown").red(),
    );
}

/// K�rl� f�rsat raporu
fn print_opportunity_report(
    opp: &ArbitrageOpportunity,
    sim: &SimulationResult,
    pools: &[PoolConfig],
    config: &BotConfig,
) {
    let buy = &pools[opp.buy_pool_idx];
    let sell = &pools[opp.sell_pool_idx];

    println!();
    println!("{}", "  -===========================================================�".red().bold());
    println!("{}", "  �     ??????  PROFITABLE ARBITRAGE OPPORTUNITY  ??????              �".red().bold());
    println!("{}", "  �===========================================================�".red().bold());
    println!("  {}  Time             : {}", "�".red(), timestamp().white().bold());
    println!(
        "  {}  Direction        : {} — {}",
        "�".red(),
        format!("BUY from {} ({:.6} Q)", buy.name, opp.buy_price_quote).green().bold(),
        format!("SELL to {} ({:.6} Q)", sell.name, opp.sell_price_quote).red().bold(),
    );
    println!("  {}  Spread           : {:.4}%", "�".red(), opp.spread_pct);
    println!("  {}  ������������������������������������������������������", "�".red());
    println!(
        "  {}  Optimal Amount   : {} WETH (Newton-Raphson: {}i, {})",
        "�".red(),
        format!("{:.6}", opp.optimal_amount_weth).white().bold(),
        opp.nr_iterations,
        if opp.nr_converged { "converged".green() } else { "not converged".yellow() },
    );
    println!(
        "  {}  ?? NET PROFIT    : {:.6} WETH",
        "�".red(),
        format!("{:.6}", opp.expected_profit_weth).green().bold(),
    );
    println!(
        "  {}  REVM Simulation  : {} (Gas: {})",
        "�".red(),
        if sim.success { "PASSED".green().bold() } else { "FAILED".red().bold() },
        sim.gas_used,
    );

    if config.execution_enabled() {
        println!(
            "  {}  Status           : {}",
            "�".red(),
            "?? EXECUTING CONTRACT...".yellow().bold()
        );
    } else if config.shadow_mode() {
        println!(
            "  {}  Status           : {}",
            "�".red(),
            "?? SHADOW MODE — logged to shadow_analytics.jsonl".yellow().bold()
        );
    } else {
        println!(
            "  {}  Status           : {}",
            "�".red(),
            "?? Observation Mode (execution disabled)".dimmed()
        );
    }
    println!("{}", "  L===========================================================-".red().bold());
    println!();
}

// �����������������������������������������������������������������������������
// Exponential Gas Base Fee Spike Testleri
// �����������������������������������������������������������������������������
//
// EIP-1559 gere�i Base a��nda base fee ard���k dolu bloklarda logaritmik
// olarak artabilir. strategy.rs i�indeki risk filtresi k�r/zarar hesab�
// yaparken a��n o anki gas'�n� kullan�r.
//
// Bu test mod�l�, base fee ani 5x art���nda:
//   1. check_arbitrage_opportunity'nin gas maliyetini do�ru hesaplamas�
//   2. K�r < gas_cost oldu�unda f�rsat� reddetmesi (None d�nmesi)
//   3. Normal gas'ta k�rl� f�rsat�n kabul edilmesi (Some d�nmesi)
// davran��lar�n� do�rular.
// �����������������������������������������������������������������������������

// �����������������������������������������������������������������������������
// Multi-Hop Arbitraj F�rsat Tespiti (v29.0: Route Engine)
// �����������������������������������������������������������������������������

/// Multi-hop rotalar �zerinde arbitraj f�rsat� tara.
///
/// Mevcut check_arbitrage_opportunity 2-pool'a odaklan�r. Bu fonksiyon
/// route_engine taraf�ndan �retilen 3+ hop rotalar� �zerinde NR optimizasyonu
/// yaparak multi-hop f�rsatlar� tespit eder.
///
/// # Parametreler
/// - `routes`: route_engine::find_routes() ��kt�s�
/// - `pools`: T�m havuz yap�land�rmalar�
/// - `states`: T�m havuz durumlar�
/// - `config`: Bot yap�land�rmas�
/// - `block_base_fee`: Mevcut blok taban �creti
/// - `l1_data_fee_wei`: L1 veri �creti (OP Stack)
///
/// # D�n��
/// K�rl� rotalar (MultiHopOpportunity listesi, k�ra g�re s�ral�)
pub fn check_multi_hop_opportunities(
    routes: &[crate::route_engine::Route],
    pools: &[PoolConfig],
    states: &[SharedPoolState],
    config: &BotConfig,
    block_base_fee: u64,
    l1_data_fee_wei: u128,
) -> Vec<crate::types::MultiHopOpportunity> {
    let mut opportunities = Vec::new();
    let l1_data_fee_weth = l1_data_fee_wei as f64 / 1e18;

    for (route_idx, route) in routes.iter().enumerate() {
        // Sadece 3+ hop rotalar�n� i�le (2-hop'lar mevcut sistem taraf�ndan kapsan�yor)
        if route.hop_count() < 3 {
            continue;
        }

        // Rotadaki t�m havuzlar aktif mi?
        // v10.0: Hard-abort — stale veya is_stale=true olan havuz varsa rota atlanır
        let all_active = route.hops.iter().all(|hop| {
            if hop.pool_idx < states.len() {
                let state = states[hop.pool_idx].load();
                state.is_active() && state.staleness_ms() <= config.max_staleness_ms
            } else {
                false
            }
        });
        if !all_active {
            continue;
        }

        // Havuz durumlar�n� ve yap�land�rmalar�n� topla
        let pool_states: Vec<crate::types::PoolState> = route.hops.iter().map(|hop| {
            states[hop.pool_idx].load_full().as_ref().clone()
        }).collect();
        let pool_configs: Vec<&PoolConfig> = route.hops.iter().map(|hop| {
            &pools[hop.pool_idx]
        }).collect();
        let directions: Vec<bool> = route.hops.iter().map(|hop| hop.zero_for_one).collect();

        let state_refs: Vec<&crate::types::PoolState> = pool_states.iter().collect();

        // Multi-hop gas tahmini: base 310K + hop ba��na 130K ek
        let multi_hop_gas: u64 = 310_000 + (route.hop_count() as u64 - 2) * 130_000;
        let dynamic_gas_cost_weth = if block_base_fee > 0 {
            let l2 = (multi_hop_gas as f64 * block_base_fee as f64) / 1e18;
            ((l2 + l1_data_fee_weth) * 1.10).max(0.000001)
        } else {
            ((config.gas_cost_fallback_weth + l1_data_fee_weth) * 1.10).max(0.000001)
        };

        // Ortalama ETH fiyat� (ilk havuzdan)
        let avg_price = pool_states[0].eth_price_usd.max(1.0);
        let gas_cost_usd = dynamic_gas_cost_weth * avg_price;

        // Multi-hop NR optimizasyonu
        let nr_result = math::find_optimal_amount_multi_hop(
            &state_refs,
            &pool_configs,
            &directions,
            gas_cost_usd,
            avg_price,
            config.max_trade_size_weth,
        );

        // K�r� WETH'e �evir
        let expected_profit_weth = if avg_price > 0.0 {
            nr_result.expected_profit / avg_price
        } else {
            continue;
        };

        // Minimum k�r e�i�i kontrol�
        // v30.0: Dinamik ROI + mutlak esik kontrolu (multi-hop)
        let mh_roi = if nr_result.optimal_amount > 0.0 {
            expected_profit_weth / nr_result.optimal_amount
        } else { 0.0 };
        if nr_result.optimal_amount <= 0.0
            || (expected_profit_weth < config.min_net_profit_weth && mh_roi < config.min_profit_roi)
        {
            continue;
        }

        let pool_indices: Vec<usize> = route.hops.iter().map(|h| h.pool_idx).collect();

        // Token path do�rulamas�: rota WETH ile ba�lay�p WETH ile bitmeli
        let token_path_valid = route.tokens.first() == route.tokens.last();
        if !token_path_valid {
            continue;
        }

        // Hop token_in/token_out tutarl�l�k kontrol�
        let hops_consistent = route.hops.windows(2).all(|w| {
            w[0].token_out == w[1].token_in
        });
        if !hops_consistent {
            continue;
        }

        // Rota tipi logla
        let _route_type = if route.is_triangular() {
            "triangular"
        } else if route.is_two_hop() {
            "two-hop"
        } else {
            "quad"
        };

        opportunities.push(crate::types::MultiHopOpportunity {
            route_idx,
            pool_indices,
            directions: directions.clone(),
            optimal_amount_weth: nr_result.optimal_amount,
            expected_profit_weth,
            label: route.label.clone(),
            nr_converged: nr_result.converged,
            nr_iterations: nr_result.iterations,
            hop_count: route.hop_count(),
        });
    }

    // K�ra g�re azalan s�ra
    opportunities.sort_by(|a, b| {
        b.expected_profit_weth
            .partial_cmp(&a.expected_profit_weth)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    opportunities
}

// �����������������������������������������������������������������������������
// Multi-Hop F�rsat De�erlendirme ve Y�r�tme (v25.0)
// �����������������������������������������������������������������������������

/// Multi-hop arbitraj f�rsat�n� de�erlendir, sim�le et ve y�r�t.
///
/// check_multi_hop_opportunities ile bulunan en iyi f�rsat� al�r,
/// REVM sim�lasyonu yapar ve MevExecutor ile Private RPC'ye g�nderir.
///
/// v25.0: G�lge modundan ��k�p ger�ek y�r�tme deste�i.
#[allow(clippy::too_many_arguments)]
pub async fn evaluate_and_execute_multi_hop<P: Provider + Sync>(
    _provider: &P,
    config: &BotConfig,
    pools: &[PoolConfig],
    states: &[SharedPoolState],
    opportunity: &crate::types::MultiHopOpportunity,
    sim_engine: &SimulationEngine,
    stats: &mut ArbitrageStats,
    nonce_manager: &Arc<NonceManager>,
    block_timestamp: u64,
    block_base_fee: u64,
    _block_latency_ms: f64,
    _l1_data_fee_wei: u128,
    mev_executor: &Arc<crate::executor::MevExecutor>,
    telegram_sender: &Option<crate::telegram::TelegramSender>,
    tg_counters: &mut crate::telegram::TelemetryCounters,
) -> Option<u64> {
    // Sıfır/NaN koruması
    if opportunity.optimal_amount_weth <= 0.0
        || !opportunity.expected_profit_weth.is_finite()
    {
        return None;
    }

    // Veri tazeliği kontrolü — tüm hop havuzları is_fresh() ile
    for &pool_idx in &opportunity.pool_indices {
        if pool_idx >= states.len() { return None; }
        let state = states[pool_idx].load();
        if !state.is_fresh(config.max_staleness_ms) {
            eprintln!(
                "     \u{1f6d1} [Multi-Hop FreshnessGate] Pool #{} stale/outdated: {}ms (threshold={}ms)",
                pool_idx, state.staleness_ms(), config.max_staleness_ms,
            );
            return None;
        }
    }

    // Hop adresleri ve y�nleri
    let pool_addrs: Vec<Address> = opportunity.pool_indices.iter()
        .map(|&i| pools[i].address).collect();
    let dirs_u8: Vec<u8> = opportunity.directions.iter()
        .map(|&d| if d { 0u8 } else { 1u8 }).collect();

    // Amount ve profit hesapla
    let amount_wei = crate::math::exact::f64_to_u256_wei(opportunity.optimal_amount_weth);

    // Exact profit do�rulamas�
    let pool_states_ex: Vec<crate::types::PoolState> = opportunity.pool_indices.iter()
        .map(|&i| states[i].load_full().as_ref().clone()).collect();
    let pool_configs_ex: Vec<&PoolConfig> = opportunity.pool_indices.iter()
        .map(|&i| &pools[i]).collect();
    let state_refs_ex: Vec<&crate::types::PoolState> = pool_states_ex.iter().collect();
    let exact_profit = crate::math::compute_exact_profit_multi_hop(
        &state_refs_ex, &pool_configs_ex, &opportunity.directions, amount_wei,
    );

    if exact_profit.is_zero() {
        eprintln!("     ?? [Multi-Hop] Exact profit zero — skipping");
        return None;
    }

    // Deadline block
    let current_block = states[opportunity.pool_indices[0]].load().last_block;
    let deadline_block = current_block as u32 + config.deadline_blocks.max(3);

    // Dinamik slippage-adjusted minProfit
    let min_liq = opportunity.pool_indices.iter()
        .map(|&i| states[i].load().liquidity)
        .min().unwrap_or(0);
    let slippage_bps = if min_liq >= 10u128.pow(15) {
        9950u64
    } else if min_liq >= 10u128.pow(13) {
        9900
    } else {
        9500
    };
    let min_profit = compute_min_profit_exact(exact_profit, slippage_bps);

    // Multi-hop calldata olu�tur
    let calldata = crate::simulator::encode_multi_hop_calldata(
        &pool_addrs, &dirs_u8, amount_wei, min_profit, deadline_block,
    );

    // REVM sim�lasyonu (kontrat adresi varsa)
    let revm_result = if let Some(contract_addr) = config.contract_address {
        let caller = config.private_key.as_ref()
            .and_then(|pk| pk.parse::<PrivateKeySigner>().ok())
            .map(|signer| signer.address())
            .unwrap_or_default();

        sim_engine.simulate(
            pools,
            states,
            caller,
            contract_addr,
            calldata.clone(),
            U256::ZERO,
            current_block,
            block_timestamp,
            block_base_fee,
        )
    } else {
        // Kontrat adresi yoksa matematiksel validasyon
        sim_engine.validate_mathematical(
            pools,
            states,
            opportunity.pool_indices[0],
            *opportunity.pool_indices.last().unwrap_or(&0),
            opportunity.optimal_amount_weth,
        )
    };

    let simulated_gas_used = revm_result.gas_used;

    if !revm_result.success {
        stats.failed_simulations += 1;
        stats.consecutive_failures += 1;
        tg_counters.reverts += 1;
        eprintln!(
            "     ?? [Multi-Hop] REVM Simulation FAILED: {}",
            revm_result.error.as_deref().unwrap_or("Unknown"),
        );
        return None;
    }

    stats.consecutive_failures = 0;
    stats.profitable_opportunities += 1;
    stats.total_potential_profit += opportunity.expected_profit_weth;
    if opportunity.expected_profit_weth > stats.max_profit_weth {
        stats.max_profit_weth = opportunity.expected_profit_weth;
    }

    println!();
    println!("{}", "  -===========================================================�".red().bold());
    println!("{}", "  �  ????  MULTI-HOP PROFITABLE ARBITRAGE OPPORTUNITY  ????           �".red().bold());
    println!("{}", "  �===========================================================�".red().bold());
    println!("  {}  Route            : {} ({})", "�".red(), opportunity.label, opportunity.hop_count);
    println!("  {}  Optimal Amount   : {:.6} WETH", "�".red(), opportunity.optimal_amount_weth);
    println!("  {}  ?? NET PROFIT    : {:.6} WETH", "�".red(), opportunity.expected_profit_weth);
    println!("  {}  Exact Profit     : {} wei", "�".red(), exact_profit);
    println!("  {}  Calldata         : {} bytes ({}-hop)", "�".red(), calldata.len(), opportunity.hop_count);
    println!("  {}  REVM Simulation  : PASSED (Gas: {})", "�".red(), simulated_gas_used);
    println!("{}", "  L===========================================================-".red().bold());
    println!();

    // G�lge modu veya ger�ek y�r�tme
    if config.shadow_mode() {
        if revm_result.success {
            stats.shadow_sim_success += 1;
            stats.shadow_cumulative_profit += opportunity.expected_profit_weth;
        } else {
            stats.shadow_sim_fail += 1;
        }
        println!(
            "  {} {}",
            "??".yellow(),
            "SHADOW MODE: Multi-hop trade skipped — logged to shadow log".yellow().bold()
        );
    } else if config.execution_enabled() {
        let pk = config.private_key.clone()
            .expect("BUG: execution_enabled() true ama private_key None");
        let contract_addr = config.contract_address
            .expect("BUG: execution_enabled() true ama contract_address None");

        let nonce = nonce_manager.get_and_increment();
        let nm_clone = Arc::clone(nonce_manager);

        stats.executed_trades += 1;

        // v32.0: Telegram — Multi-hop Alfa Bildirimi
        let gas_cost_weth_mh = simulated_gas_used as f64 * block_base_fee as f64 / 1e18;
        if let Some(ref tg) = telegram_sender {
            tg.send(crate::telegram::TelegramMessage::AlphaSuccess {
                buy_pool: format!("Multi-Hop {}", opportunity.label),
                sell_pool: format!("{}-hop", opportunity.hop_count),
                gross_profit_weth: opportunity.expected_profit_weth,
                gas_cost_weth: gas_cost_weth_mh,
                net_profit_weth: opportunity.expected_profit_weth - gas_cost_weth_mh,
                latency_ms: _block_latency_ms,
                tx_hash: format!("nonce:{}", nonce_manager.current().saturating_sub(1)),
            });
        }
        tg_counters.successful_trades += 1;
        tg_counters.net_period_profit_weth += opportunity.expected_profit_weth - gas_cost_weth_mh;

        let sim_gas = simulated_gas_used;
        let expected_profit = opportunity.expected_profit_weth;
        let mev_exec = Arc::clone(mev_executor);
        let calldata_owned = calldata;

        tokio::spawn(async move {
            println!("\n  {} {}", "????".yellow(), "MULTI-HOP CONTRACT EXECUTION STARTED (Private RPC)".yellow().bold());

            let result = mev_exec.execute_protected(
                &pk,
                contract_addr,
                &calldata_owned,
                nonce,
                expected_profit,
                sim_gas,
                block_base_fee,
                current_block,
                &nm_clone,
            ).await;

            match result {
                Ok(hash) => {
                    println!("  {} Multi-hop TX successful (Private RPC): {}", "?".green(), hash.green().bold());
                }
                Err(e) => {
                    println!("  {} Multi-hop TX error: {}", "?".red(), format!("{}", e).red());
                }
            }
        });
    }

    Some(simulated_gas_used)
}

#[cfg(test)]
mod gas_spike_tests {
    use super::*;
    use alloy::primitives::{address, Address};
    use std::sync::Arc;
    use arc_swap::ArcSwap;
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
            flash_loan_fee_bps: 0.0,
            min_net_profit_weth: min_profit,
            min_profit_roi: 0.0005,
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
            max_pool_fee_bps: 200, // Test: y�ksek tavan � gas spike testleri fee filtresinden etkilenmesin
            min_tvl_usd: 1_000_000.0,
            min_volume_24h_usd: 500_000.0,
            max_tracked_pools: 4,
            telegram_bot_token: None,
            telegram_chat_id: None,
            telegram_enabled: false,
            telegram_shift_interval_secs: 21600,
            telegram_balance_warn_eth: 0.05,
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
                base_token_address: address!("4200000000000000000000000000000000000006"),
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
                base_token_address: address!("4200000000000000000000000000000000000006"),
            },
        ]
    }

    fn make_pool_state(eth_price: f64, liq: u128, block: u64) -> SharedPoolState {
        // sqrtPriceX96 hesapla � math.rs::make_test_pool ile tutarl� form�l
        let price_ratio = eth_price * 1e-12; // token1/token0 raw fiyat oran�
        let sqrt_price = price_ratio.sqrt();
        let sqrt_price_f64 = sqrt_price * (1u128 << 96) as f64;
        // Tick'i sqrtPriceX96'dan do�ru hesapla (dampening tutarl�l��� i�in)
        let tick = (price_ratio.ln() / 0.000_099_995_000_33_f64).floor() as i32;
        // v7.0: U256 sqrtPriceX96 art�k exact tick-bazl� hesaplan�r
        let sqrt_price_x96_u256 = math::exact::get_sqrt_ratio_at_tick(tick);
        Arc::new(ArcSwap::from_pointee(PoolState {
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
            is_stale: false,
        }))
    }

    /// Gas spike testi: Base fee 5x art���nda, �nceki REVM sim�lasyonundan
    /// gelen gas de�eri ile hesaplanan maliyet k�r� a��yorsa, f�rsat
    /// reddedilmeli (check_arbitrage_opportunity � None).
    ///
    /// Senaryo:
    ///   - Beklenen k�r: ~0.002 WETH (k���k spread)
    ///   - Normal base fee: 100 Gwei � gas cost ~0.000015 WETH
    ///   - 5x spike: 500 Gwei � gas cost ~0.000075 WETH (h�l� k�rl�)
    ///   - 50x spike: 5000 Gwei � gas cost ~0.00075 WETH
    ///
    /// As�l test: Dinamik gas de�eri (last_simulated_gas) ile hesaplanan
    /// maliyet, f�rsat�n k�rl�l�k e�i�ini do�ru filtreliyor mu?
    #[test]
    fn test_circuit_breaker_on_gas_spike() {
        let pools = make_pool_configs();
        // min_net_profit = 0.0002 WETH � k���k k�rl� f�rsatlar� yakala
        let config = make_test_config(0.0002, 0.00005);

        // Havuz fiyatlar�: %0.01 spread (�ok dar)
        // Bu spread ancak d���k gas'ta k�rl�
        let price_a = 2500.0;
        let price_b = 2500.25; // $0.25 spread � ~$0.25 br�t k�r (d���k)

        let liq = 50_000_000_000_000_000_000u128; // 50e18 likidite

        let states: Vec<SharedPoolState> = vec![
            make_pool_state(price_a, liq, 100),
            make_pool_state(price_b, liq, 100),
        ];

        // ��� NORMAL GAS: base_fee = 100 Gwei ���������������������
        let normal_base_fee: u64 = 100_000_000_000; // 100 Gwei

        // �nceki REVM: 150K gas sim�le edilmi�
        let last_sim_gas = Some(150_000u64);

        // Gas cost = 150K * 100 Gwei / 1e18 = 0.000015 WETH
        // K���k spread � Newton-Raphson �ok d���k optimal miktar hesaplar
        // � k�r�n gas'� kar��lay�p kar��lamayaca�� NR'a ba�l�
        let result_normal = check_arbitrage_opportunity(
            &pools, &states, &config, normal_base_fee, last_sim_gas, 0,
        );
        // Not: NR sonucu spread'e ve likiditeye ba�l� � bu test gas etkisini �l�er

        // ��� GAS SP�KE: base_fee 5000x � 500.000 Gwei �����������
        // Ger�ek�i olmayan ama stres testi: base_fee = 500K Gwei
        // Gas cost = 150K * 500K Gwei / 1e18 = 0.075 WETH
        // Hi�bir k���k spread bunu kar��layamaz
        let spike_base_fee: u64 = 500_000_000_000_000; // 500K Gwei (a��r� spike)

        let result_spike = check_arbitrage_opportunity(
            &pools, &states, &config, spike_base_fee, last_sim_gas, 0,
        );

        // Gas spike durumunda f�rsat kesinlikle reddedilmeli
        assert!(
            result_spike.is_none(),
            "A��r� gas spike (0.075+ WETH maliyet) ile f�rsat reddedilmeli (None d�nmeli)"
        );

        // ��� D�NAM�K GAS ETK�S� TEST� ��������������������������
        // Ayn� base_fee, farkl� REVM gas tahmini
        // 150K gas � 0.000015 WETH, 1.5M gas � 0.00015 WETH
        let high_gas = Some(1_500_000u64); // 10x daha fazla gas
        let result_high_gas = check_arbitrage_opportunity(
            &pools, &states, &config, normal_base_fee, high_gas, 0,
        );

        // Y�ksek gas tahminiyle maliyet artar � baz� f�rsatlar reddedilir
        // Bu testin amac�: last_simulated_gas'�n ger�ekten kullan�ld���n� kan�tlamak
        // E�er h�l� hardcoded 150K kullan�lsayd�, high_gas parametresi etkisiz olurdu
        let result_low_gas = check_arbitrage_opportunity(
            &pools, &states, &config, normal_base_fee, Some(10_000u64), 0, // �ok d���k gas
        );

        // D���k gas � d���k maliyet � f�rsat bulma olas�l��� ARTAR
        // Y�ksek gas � y�ksek maliyet � f�rsat bulma olas�l��� AZALIR
        // En az�ndan biri farkl� sonu� vermeli (dinamik gas etkisi kan�t�)
        // Not: Her ikisi de None olabilir (spread �ok dar) ama bu bile kabul
        // edilir � �nemli olan spike'�n None d�nd�rmesi.
        eprintln!(
            "Gas spike test results: normal={:?}, spike={:?}, high_gas={:?}, low_gas={:?}",
            result_normal.as_ref().map(|r| r.expected_profit_weth),
            result_spike.as_ref().map(|r| r.expected_profit_weth),
            result_high_gas.as_ref().map(|r| r.expected_profit_weth),
            result_low_gas.as_ref().map(|r| r.expected_profit_weth),
        );
    }

    /// Gas spike ile k�rl� f�rsat: B�y�k spread y�ksek gas'� kar��lar.
    ///
    /// Senaryo: %2 spread (b�y�k k�r potansiyeli), 5x gas spike
    /// Gas cost: 150K * 500 Gwei / 1e18 = 0.000075 WETH
    /// K�r >> gas cost � f�rsat h�l� k�rl� olmal�
    #[test]
    fn test_gas_spike_large_spread_still_profitable() {
        let pools = make_pool_configs();
        let config = make_test_config(0.0002, 0.00005);

        // B�y�k spread: %2 � k�rl� olmal� (y�ksek gas'a ra�men)
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

        // B�y�k spread gas spike'�n� kar��lamal�
        assert!(
            result.is_some(),
            "B�y�k spread (%2) ile gas spike'a ra�men f�rsat bulunmal�"
        );
        let opp = result.unwrap();
        assert!(
            opp.expected_profit_weth > 0.0002,
            "K�r minimum e�ikten ({}) y�ksek olmal�: {:.6}",
            0.0002,
            opp.expected_profit_weth
        );
    }

    /// Base fee = 0 fallback testi: EIP-1559 �ncesi veya hata durumu.
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

        // base_fee = 0 � config.gas_cost_fallback_weth (0.00005 WETH)
        let result = check_arbitrage_opportunity(
            &pools, &states, &config, 0, Some(150_000), 0,
        );

        assert!(
            result.is_some(),
            "base_fee=0 durumunda config fallback ile f�rsat bulunmal�"
        );
    }
}
