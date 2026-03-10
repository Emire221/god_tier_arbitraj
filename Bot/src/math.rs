// ============================================================================
//  MATH v7.0 — U256 Exact-Math Stabilizasyon + Multi-Tick CL Swap Motoru
//
//  v7.0 Yenilikler (Faz 1 — Stabilizasyon):
//  ✓ compute_arbitrage_profit_with_bitmap → U256 exact::compute_exact_swap
//  ✓ find_optimal_amount_with_bitmap → U256 liq cap (max_safe_swap_amount_u256)
//  ✓ swap_weth_to_usdc_exact / swap_usdc_to_weth_exact — U256 public API
//  ✓ max_safe_swap_amount → U256 delegasyonu
//  ✓ Uniswap V3 FullMath/SqrtPriceMath/SwapMath birebir Rust U256 port'u
//  ✓ Newton-Raphson optimizer artık U256 swap ile profit değerlendirmesi yapar
//  ✓ On-chain sonuçla wei bazında eşleşen deterministik kesinlik
//
//  v6.0 (korunuyor):
//  ✓ GERÇEK multi-tick swap: TickBitmap verisinden sıralı tick geçişi (legacy f64)
//  ✓ "50 ETH satarsam hangi 3 tick'i patlatırım?" → mikrosaniye cevap
//  ✓ Her tick sınırında liquidityNet ile aktif likidite güncelleme
//  ✓ Fallback: TickBitmap yoksa eski dampening moduna geç
//
//  v5.1 (korunuyor):
//  ✓ Tick ↔ Fiyat çift yönlü dönüşüm ve çapraz doğrulama
//  ✓ Token sırası farkındalığı (token0_is_weth — Base: WETH < USDC)
//  ✓ Newton-Raphson'a likidite-tabanlı üst sınır ve tick-impact freni
// ============================================================================

use crate::types::{PoolState, TickBitmapData};

// ─────────────────────────────────────────────────────────────────────────────
// O(1) PreFilter — NR'den Önce Hızlı Kârlılık Eleme
// ─────────────────────────────────────────────────────────────────────────────

/// O(1) kârlılık ön filtresi.
///
/// Newton-Raphson (NR) optimizasyonunun ~40 iterasyonluk kaba tarama +
/// ~50 iterasyonluk ince ayar maliyetini önlemek için, spread'in
/// fee'leri kurtarıp kurtaramayacağını tek bir çarpma/çıkarma ile kontrol eder.
///
/// Formül:
///   expected_profit = (spread_ratio × amount) - (fee_a + fee_b + gas_cost)
///
/// `expected_profit > min_profit_wei` değilse NR'ye hiç girmeden `None` döner.
pub struct PreFilter {
    /// Havuz A fee oranı (ör: 0.0005 = %0.05)
    pub fee_a: f64,
    /// Havuz B fee oranı (ör: 0.0001 = %0.01)
    pub fee_b: f64,
    /// Tahmini gas maliyeti (WETH cinsinden) — L2 + L1 + güvenlik marjı
    pub estimated_gas_cost_weth: f64,
    /// Minimum kâr eşiği (WETH cinsinden)
    pub min_profit_weth: f64,
    /// Flash loan fee oranı (ör: 0.0005 = 5 bps)
    pub flash_loan_fee_rate: f64,
    /// Builder bribe yüzdesi (ör: 0.25 = %25)
    /// v19.0: Brüt kârdan bribe düşüldükten sonra net kâr hesaplanır
    pub bribe_pct: f64,
}

/// PreFilter sonucu
#[derive(Debug, Clone, Copy)]
pub enum PreFilterResult {
    /// Spread fee'leri kurtarıyor — NR'ye devam et
    Profitable {
        /// Tahmini brüt kâr (WETH)
        estimated_profit_weth: f64,
        /// Spread oranı
        spread_ratio: f64,
    },
    /// Spread fee'leri kurtaramıyor — NR'yi atla
    Unprofitable {
        /// Neden kârsız?
        reason: PreFilterRejectReason,
    },
}

/// Kârsızlık nedeni (debug logları için)
#[derive(Debug, Clone, Copy)]
pub enum PreFilterRejectReason {
    /// Spread toplam fee'den küçük
    SpreadBelowFees,
    /// Tahmini kâr minimum eşiğin altında
    ProfitBelowThreshold,
    /// Geçersiz fiyat verisi
    InvalidPriceData,
}

impl PreFilter {
    /// O(1) kârlılık kontrolü.
    ///
    /// # Argümanlar
    /// - `price_a`: Havuz A ETH fiyatı (quote cinsinden)
    /// - `price_b`: Havuz B ETH fiyatı (quote cinsinden)
    /// - `trade_amount_weth`: İşlem boyutu (WETH)
    ///
    /// # Karmaşıklık
    /// O(1) — sabit sayıda aritmetik operasyon, allocation yok.
    #[inline]
    pub fn check(
        &self,
        price_a: f64,
        price_b: f64,
        trade_amount_weth: f64,
    ) -> PreFilterResult {
        // Geçerlilik kontrolü — NaN/Infinity/sıfır fiyat
        if price_a <= 0.0 || price_b <= 0.0
            || !price_a.is_finite() || !price_b.is_finite()
            || trade_amount_weth <= 0.0
        {
            return PreFilterResult::Unprofitable {
                reason: PreFilterRejectReason::InvalidPriceData,
            };
        }

        // Spread oranı = |price_a - price_b| / min(price_a, price_b)
        let spread = (price_a - price_b).abs();
        let min_price = price_a.min(price_b);
        let spread_ratio = spread / min_price;

        // Toplam fee oranı = fee_a + fee_b + flash_loan_fee
        let total_fee_ratio = self.fee_a + self.fee_b + self.flash_loan_fee_rate;

        // Spread fee'leri kurtarıyor mu?
        if spread_ratio <= total_fee_ratio {
            return PreFilterResult::Unprofitable {
                reason: PreFilterRejectReason::SpreadBelowFees,
            };
        }

        // v19.0: Tahmini net kâr (WETH cinsinden)
        // Brüt kâr = (spread_ratio - total_fee_ratio) × amount
        // Net kâr = brüt_kâr × (1 - bribe_pct) - gas_cost
        // Bu formül komisyon + gas + bribe'ı tek seferde değerlendirir.
        let gross_profit = (spread_ratio - total_fee_ratio) * trade_amount_weth;
        let net_after_bribe = gross_profit * (1.0 - self.bribe_pct);
        let estimated_profit = net_after_bribe - self.estimated_gas_cost_weth;

        if estimated_profit < self.min_profit_weth {
            return PreFilterResult::Unprofitable {
                reason: PreFilterRejectReason::ProfitBelowThreshold,
            };
        }

        PreFilterResult::Profitable {
            estimated_profit_weth: estimated_profit,
            spread_ratio,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Sabitler
// ─────────────────────────────────────────────────────────────────────────────

/// 2^96 — sqrtPriceX96 çözümleme sabiti
const Q96: f64 = 79_228_162_514_264_337_593_543_950_336.0;

/// WETH decimals (10^18)
const WETH_DECIMALS: f64 = 1_000_000_000_000_000_000.0;

/// USDC decimals (10^6)
const USDC_DECIMALS: f64 = 1_000_000.0;

/// ln(1.0001) — tick ↔ fiyat dönüşümü için
const LOG_TICK_BASE: f64 = 0.000_099_995_000_33;

/// Tick sınırı geçiş dampening faktörü (FALLBACK modu).
/// TickBitmap yoksa her tick_spacing sınırında bu uygulanır.
const TICK_CROSS_DAMPENING: f64 = 0.997;

/// Likidite güvenlik marjı — swap miktarı, havuz kapasitesinin
/// bu oranını aşmamalı (tick-çapraz hataları önlemek için)
/// ⚠️ LEGACY: U256 path exact::max_safe_swap_amount_u256 kullanır.
#[allow(dead_code)]
const MAX_LIQUIDITY_USAGE_RATIO: f64 = 0.15;

// ─────────────────────────────────────────────────────────────────────────────
// Güvenlik Yardımcıları
// ─────────────────────────────────────────────────────────────────────────────

/// f64 değerini temizle: NaN veya Infinity ise 0.0 döndür.
/// Canlı MEV botunun çökmesini engelleyen kritik güvenlik katmanı.
#[inline]
fn sanitize_f64(v: f64) -> f64 {
    if v.is_nan() || v.is_infinite() { 0.0 } else { v }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tick ↔ Fiyat Dönüşümleri
// ─────────────────────────────────────────────────────────────────────────────

/// Tick'i ham fiyat oranına (token1_raw / token0_raw) çevir.
#[inline]
pub fn tick_to_price_ratio(tick: i32) -> f64 {
    (tick as f64 * LOG_TICK_BASE).exp()
}

/// Tick'i sqrtPriceX96 değerine çevir (doğrulama amaçlı).
#[inline]
#[allow(dead_code)]
pub fn tick_to_sqrt_price_x96(tick: i32) -> f64 {
    let half_tick = tick as f64 * 0.5;
    (half_tick * LOG_TICK_BASE).exp() * Q96
}

/// sqrtPriceX96'dan tick hesapla.
#[inline]
pub fn sqrt_price_x96_to_tick(sqrt_price_x96: f64) -> i32 {
    if sqrt_price_x96 <= 0.0 || sqrt_price_x96.is_nan() || sqrt_price_x96.is_infinite() {
        return 0;
    }
    let sqrt_price = sqrt_price_x96 / Q96;
    let price = sqrt_price * sqrt_price;
    if price <= 0.0 || price.is_nan() || price.is_infinite() {
        return 0;
    }
    let raw_tick = price.ln() / LOG_TICK_BASE;
    if raw_tick.is_nan() || raw_tick.is_infinite() {
        return 0;
    }
    // Uniswap V3 geçerli tick aralığına sınırla
    (raw_tick.floor() as i64).clamp(-887272, 887272) as i32
}

/// Ham fiyat oranını ETH/USDC fiyatına çevir (token sırası farkındalığı ile).
#[inline]
fn raw_price_to_eth_price(
    price_ratio: f64,
    token0_decimals: u8,
    token1_decimals: u8,
    token0_is_weth: bool,
) -> f64 {
    if price_ratio.is_nan() || price_ratio.is_infinite() || price_ratio <= 0.0 {
        return 0.0;
    }
    let decimal_adj = 10.0_f64.powi(token0_decimals as i32 - token1_decimals as i32);

    let result = if token0_is_weth {
        price_ratio * decimal_adj
    } else {
        let adjusted = price_ratio * decimal_adj;
        if adjusted > 1e-300 { 1.0 / adjusted } else { 0.0 }
    };
    // NaN veya Infinity asla dışarı sızdırma
    if result.is_nan() || result.is_infinite() { 0.0 } else { result }
}

/// Tick'ten ETH fiyatını (USDC cinsinden) hesapla.
#[allow(dead_code)]
pub fn tick_to_eth_price(
    tick: i32,
    token0_decimals: u8,
    token1_decimals: u8,
    token0_is_weth: bool,
) -> f64 {
    let price_ratio = tick_to_price_ratio(tick);
    raw_price_to_eth_price(price_ratio, token0_decimals, token1_decimals, token0_is_weth)
}

// ─────────────────────────────────────────────────────────────────────────────
// Ana Fiyat Hesaplama — Tick Doğrulamalı
// ─────────────────────────────────────────────────────────────────────────────

/// sqrtPriceX96 + tick çapraz doğrulaması ile ETH fiyatı hesapla.
pub fn compute_eth_price(
    sqrt_price_x96: f64,
    tick: i32,
    token0_decimals: u8,
    token1_decimals: u8,
    token0_is_weth: bool,
) -> f64 {
    if sqrt_price_x96 <= 0.0 || sqrt_price_x96.is_nan() || sqrt_price_x96.is_infinite() {
        return 0.0;
    }

    let sqrt_price = sqrt_price_x96 / Q96;
    let price_ratio_sqrt = sqrt_price * sqrt_price;
    let price_from_sqrt = raw_price_to_eth_price(
        price_ratio_sqrt, token0_decimals, token1_decimals, token0_is_weth,
    );

    let price_ratio_tick = tick_to_price_ratio(tick);
    let price_from_tick = raw_price_to_eth_price(
        price_ratio_tick, token0_decimals, token1_decimals, token0_is_weth,
    );

    if price_from_sqrt > 0.0 && price_from_tick > 0.0 {
        let deviation = ((price_from_sqrt - price_from_tick) / price_from_tick).abs();
        if deviation > 0.01 {
            eprintln!(
                "  ⚠️ Fiyat sapması: sqrtPrice={:.2}$, tick={:.2}$ (sapma: {:.2}%)",
                price_from_sqrt, price_from_tick, deviation * 100.0
            );
            return price_from_tick;
        }
    }

    price_from_sqrt
}

/// [Geriye Uyumluluk]
#[allow(dead_code)]
pub fn sqrt_price_x96_to_eth_price(
    sqrt_price_x96: f64,
    token0_decimals: u8,
    token1_decimals: u8,
) -> f64 {
    if sqrt_price_x96 <= 0.0 {
        return 0.0;
    }
    let sqrt_price = sqrt_price_x96 / Q96;
    let price_ratio = sqrt_price * sqrt_price;
    let decimal_diff = token1_decimals as i32 - token0_decimals as i32;
    price_ratio * 10.0_f64.powi(decimal_diff)
}

// ─────────────────────────────────────────────────────────────────────────────
// Multi-Tick Swap Sonucu
// ─────────────────────────────────────────────────────────────────────────────

/// Multi-tick swap simülasyonunun detaylı sonucu.
/// "50 ETH satarsam sırasıyla hangi 3 tick'i patlatırım ve ortalama fiyat ne olur?"
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct MultiTickSwapResult {
    /// Toplam çıktı miktarı (USDC veya WETH)
    pub total_output: f64,
    /// Efektif (ağırlıklı ortalama) fiyat
    pub effective_price: f64,
    /// Geçilen tick sınırları ve her birindeki swap detayı
    pub tick_crossings: Vec<TickCrossing>,
    /// Son tick (swap sonrası konum)
    pub final_tick: i32,
    /// Son sqrtPrice (Q96 formatında)
    pub final_sqrt_price_x96: f64,
    /// Son aktif likidite
    pub final_liquidity: f64,
    /// Gerçek TickBitmap mı yoksa dampening fallback mı kullanıldı?
    pub used_real_bitmap: bool,
}

/// Tek bir tick geçişinin detayı
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TickCrossing {
    /// Geçilen tick sınırı
    pub tick: i32,
    /// Bu aralıkta kullanılan likidite
    pub liquidity: f64,
    /// Bu aralıkta tüketilen girdi miktarı
    pub input_consumed: f64,
    /// Bu aralıktan elde edilen çıktı
    pub output_produced: f64,
    /// Net likidite değişimi (liquidityNet)
    pub liquidity_net: i128,
}

// ─────────────────────────────────────────────────────────────────────────────
// U256 Multi-Tick Swap — Public API (v7.0)
// ─────────────────────────────────────────────────────────────────────────────

/// WETH → USDC swap (U256 exact math, PoolState doğrudan).
///
/// Uniswap V3'ün on-chain swap() fonksiyonunu birebir taklit eder.
/// Sonuç f64'e dönüştürülür (human-readable USDC).
pub fn swap_weth_to_usdc_exact(
    pool: &PoolState,
    amount_in_weth: f64,
    fee_fraction: f64,
    token0_is_weth: bool,
    tick_spacing: i32,
    tick_bitmap: Option<&TickBitmapData>,
) -> MultiTickSwapResult {
    if amount_in_weth <= 0.0 || pool.liquidity == 0 || pool.sqrt_price_x96.is_zero() {
        return MultiTickSwapResult {
            total_output: 0.0,
            effective_price: 0.0,
            tick_crossings: vec![],
            final_tick: pool.tick,
            final_sqrt_price_x96: pool.sqrt_price_f64,
            final_liquidity: pool.liquidity as f64,
            used_real_bitmap: false,
        };
    }

    let amount_in_wei = alloy::primitives::U256::from(
        crate::types::safe_f64_to_u128(amount_in_weth * 1e18)
    );
    let fee_pips = exact::fee_fraction_to_pips(fee_fraction);
    let zero_for_one = token0_is_weth;

    let result = exact::compute_exact_swap(
        pool.sqrt_price_x96,
        pool.liquidity,
        pool.tick,
        amount_in_wei,
        zero_for_one,
        fee_pips,
        tick_spacing,
        tick_bitmap,
    );

    // U256 → f64 dönüşümü (USDC: / 1e6)
    let total_usdc = exact::u256_to_f64(result.amount_out) / USDC_DECIMALS;
    let effective_price = if amount_in_weth > 0.0 { total_usdc / amount_in_weth } else { 0.0 };

    MultiTickSwapResult {
        total_output: sanitize_f64(total_usdc),
        effective_price: sanitize_f64(effective_price),
        tick_crossings: vec![], // U256 path detaylı tick log tutmaz
        final_tick: pool.tick,
        final_sqrt_price_x96: exact::u256_to_f64(result.final_sqrt_price_x96),
        final_liquidity: result.final_liquidity as f64,
        used_real_bitmap: tick_bitmap.is_some(),
    }
}

/// USDC → WETH swap (U256 exact math, PoolState doğrudan).
#[allow(dead_code)]
pub fn swap_usdc_to_weth_exact(
    pool: &PoolState,
    amount_in_usdc: f64,
    fee_fraction: f64,
    token0_is_weth: bool,
    tick_spacing: i32,
    tick_bitmap: Option<&TickBitmapData>,
) -> MultiTickSwapResult {
    if amount_in_usdc <= 0.0 || pool.liquidity == 0 || pool.sqrt_price_x96.is_zero() {
        return MultiTickSwapResult {
            total_output: 0.0,
            effective_price: 0.0,
            tick_crossings: vec![],
            final_tick: pool.tick,
            final_sqrt_price_x96: pool.sqrt_price_f64,
            final_liquidity: pool.liquidity as f64,
            used_real_bitmap: false,
        };
    }

    let amount_in_wei = alloy::primitives::U256::from(
        crate::types::safe_f64_to_u128(amount_in_usdc * 1e6)
    );
    let fee_pips = exact::fee_fraction_to_pips(fee_fraction);
    let zero_for_one = !token0_is_weth; // USDC girdi yönü

    let result = exact::compute_exact_swap(
        pool.sqrt_price_x96,
        pool.liquidity,
        pool.tick,
        amount_in_wei,
        zero_for_one,
        fee_pips,
        tick_spacing,
        tick_bitmap,
    );

    // U256 → f64 dönüşümü (WETH: / 1e18)
    let total_weth = exact::u256_to_f64(result.amount_out) / WETH_DECIMALS;
    let effective_price = if total_weth > 0.0 { amount_in_usdc / total_weth } else { 0.0 };

    MultiTickSwapResult {
        total_output: sanitize_f64(total_weth),
        effective_price: sanitize_f64(effective_price),
        tick_crossings: vec![],
        final_tick: pool.tick,
        final_sqrt_price_x96: exact::u256_to_f64(result.final_sqrt_price_x96),
        final_liquidity: result.final_liquidity as f64,
        used_real_bitmap: tick_bitmap.is_some(),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// LEGACY Multi-Tick Swap Motoru (f64 — Geriye Uyumluluk)
// ─────────────────────────────────────────────────────────────────────────────

/// WETH → USDC multi-tick swap (TickBitmap ile gerçek tick geçişi).
/// ⚠️ LEGACY: f64 tabanlı. Yeni kod `swap_weth_to_usdc_exact` kullanmalıdır.
///
/// Algoritma:
///   1. Mevcut tick'teki kalan likiditeyle olabildiğince swap yap
///   2. Girdi tükenmezse, sonraki başlatılmış tick'e ilerle
///   3. O tick'te liquidityNet'i aktif likiditeye ekle/çıkar
///   4. Yeni likiditeyle swap'a devam et
///   5. Girdi tükenene veya başlatılmış tick kalmayana kadar tekrarla
///
/// token0=WETH → WETH girdi, USDC çıktı → sqrtPrice AZALIR → sola git
/// token0=USDC → WETH girdi (token1), USDC çıktı (token0) → sqrtPrice ARTAR → sağa git
pub fn swap_weth_to_usdc_multitick(
    sqrt_price_f64: f64,
    liquidity: f64,
    current_tick: i32,
    amount_in_weth: f64,
    fee_fraction: f64,
    token0_is_weth: bool,
    tick_spacing: i32,
    tick_bitmap: Option<&TickBitmapData>,
) -> MultiTickSwapResult {
    if sqrt_price_f64 <= 0.0 || liquidity <= 0.0 || amount_in_weth <= 0.0 {
        return MultiTickSwapResult {
            total_output: 0.0,
            effective_price: 0.0,
            tick_crossings: vec![],
            final_tick: current_tick,
            final_sqrt_price_x96: sqrt_price_f64,
            final_liquidity: liquidity,
            used_real_bitmap: false,
        };
    }

    // Fee'yi düş
    let total_effective = amount_in_weth * (1.0 - fee_fraction);

    match tick_bitmap {
        Some(bitmap) if !bitmap.ticks.is_empty() => {
            // ─── GERÇEK Multi-Tick Engine ─────────────────────────
            real_multitick_swap_weth_to_usdc(
                sqrt_price_f64, liquidity, current_tick,
                total_effective, token0_is_weth, tick_spacing, bitmap,
            )
        }
        _ => {
            // ─── FALLBACK: Dampening Modu ─────────────────────────
            let output = dampened_swap_weth_to_usdc(
                sqrt_price_f64, liquidity, current_tick,
                total_effective, token0_is_weth, tick_spacing,
            );
            let eff_price = if amount_in_weth > 0.0 { output / amount_in_weth } else { 0.0 };
            MultiTickSwapResult {
                total_output: sanitize_f64(output),
                effective_price: sanitize_f64(eff_price),
                tick_crossings: vec![],
                final_tick: current_tick,
                final_sqrt_price_x96: sqrt_price_f64,
                final_liquidity: liquidity,
                used_real_bitmap: false,
            }
        }
    }
}

/// USDC → WETH multi-tick swap (TickBitmap ile gerçek tick geçişi).
pub fn swap_usdc_to_weth_multitick(
    sqrt_price_f64: f64,
    liquidity: f64,
    current_tick: i32,
    amount_in_usdc: f64,
    fee_fraction: f64,
    token0_is_weth: bool,
    tick_spacing: i32,
    tick_bitmap: Option<&TickBitmapData>,
) -> MultiTickSwapResult {
    if sqrt_price_f64 <= 0.0 || liquidity <= 0.0 || amount_in_usdc <= 0.0 {
        return MultiTickSwapResult {
            total_output: 0.0,
            effective_price: 0.0,
            tick_crossings: vec![],
            final_tick: current_tick,
            final_sqrt_price_x96: sqrt_price_f64,
            final_liquidity: liquidity,
            used_real_bitmap: false,
        };
    }

    let total_effective = amount_in_usdc * (1.0 - fee_fraction);

    match tick_bitmap {
        Some(bitmap) if !bitmap.ticks.is_empty() => {
            real_multitick_swap_usdc_to_weth(
                sqrt_price_f64, liquidity, current_tick,
                total_effective, token0_is_weth, tick_spacing, bitmap,
            )
        }
        _ => {
            let output = dampened_swap_usdc_to_weth(
                sqrt_price_f64, liquidity, current_tick,
                total_effective, token0_is_weth, tick_spacing,
            );
            let eff_price = if output > 0.0 { amount_in_usdc / output } else { 0.0 };
            MultiTickSwapResult {
                total_output: sanitize_f64(output),
                effective_price: sanitize_f64(eff_price),
                tick_crossings: vec![],
                final_tick: current_tick,
                final_sqrt_price_x96: sqrt_price_f64,
                final_liquidity: liquidity,
                used_real_bitmap: false,
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Gerçek Multi-Tick Swap — İç Motor
// ─────────────────────────────────────────────────────────────────────────────

/// WETH → USDC gerçek multi-tick swap.
///
/// token0=WETH → token0 girdi → sqrtPrice AZALIR → sola (negatif tick yönünde) ilerle
/// token0=USDC → token1 girdi → sqrtPrice ARTAR → sağa (pozitif tick yönünde) ilerle
fn real_multitick_swap_weth_to_usdc(
    sqrt_price_f64: f64,
    liquidity: f64,
    current_tick: i32,
    amount_in_effective: f64,
    token0_is_weth: bool,
    _tick_spacing: i32,
    bitmap: &TickBitmapData,
) -> MultiTickSwapResult {
    // WETH girişi: human-readable (ör: 5.0) → raw (5e18)
    let mut remaining_input = amount_in_effective * WETH_DECIMALS;
    let mut total_output = 0.0;
    let mut sqrt_p = sqrt_price_f64 / Q96; // Normalize
    let mut active_liq = liquidity;
    let mut curr_tick = current_tick;
    let mut crossings: Vec<TickCrossing> = Vec::new();

    // WETH girdi yönü: token0=WETH → sola git, token0=USDC → sağa git
    let go_left = token0_is_weth;

    // Sıralı başlatılmış tick'leri al
    let ordered_ticks = if go_left {
        // Sola giderken: current_tick'ten küçük tick'ler (büyükten küçüğe)
        let mut t: Vec<_> = bitmap.ticks.iter()
            .filter(|(&tick, info)| tick <= curr_tick && info.initialized)
            .map(|(&tick, info)| (tick, *info))
            .collect();
        t.sort_by(|a, b| b.0.cmp(&a.0)); // Büyükten küçüğe
        t
    } else {
        // Sağa giderken: current_tick'ten büyük tick'ler (küçükten büyüğe)
        let mut t: Vec<_> = bitmap.ticks.iter()
            .filter(|(&tick, info)| tick > curr_tick && info.initialized)
            .map(|(&tick, info)| (tick, *info))
            .collect();
        t.sort_by_key(|(tick, _)| *tick); // Küçükten büyüğe
        t
    };

    // Güvenlik: maksimum 50 tick geçişi
    let max_crossings = 50;
    let mut crossings_count = 0;

    for (next_tick, tick_info) in &ordered_ticks {
        if remaining_input <= 1e-12 || crossings_count >= max_crossings {
            break;
        }

        // Sonraki tick sınırındaki sqrtPrice
        let next_sqrt_p = tick_to_sqrt_price_x96(*next_tick) / Q96;

        // Bu aralıkta ne kadar input tüketebiliriz?
        let (input_consumed, output_produced, new_sqrt_p) = if token0_is_weth {
            // token0 (WETH) girdi → sqrtPrice azalır
            compute_swap_within_tick_token0(sqrt_p, next_sqrt_p, active_liq, remaining_input)
        } else {
            // token1 (WETH) girdi → sqrtPrice artar
            compute_swap_within_tick_token1(sqrt_p, next_sqrt_p, active_liq, remaining_input)
        };

        if input_consumed > 0.0 {
            total_output += output_produced;
            remaining_input -= input_consumed;
            sqrt_p = new_sqrt_p;

            crossings.push(TickCrossing {
                tick: *next_tick,
                liquidity: active_liq,
                input_consumed,
                output_produced,
                liquidity_net: tick_info.liquidity_net,
            });

            // Tick sınırını geçti → likiditeyi güncelle
            if (remaining_input > 1e-12) && ((new_sqrt_p - next_sqrt_p).abs() < 1e-30) {
                // Yön: sola gidiyorsak liquidityNet'i çıkar, sağa gidiyorsak ekle
                if go_left {
                    active_liq -= tick_info.liquidity_net as f64;
                } else {
                    active_liq += tick_info.liquidity_net as f64;
                }
                active_liq = active_liq.max(0.0);
                curr_tick = *next_tick;
                crossings_count += 1;
            }
        }
    }

    // Tick arası kalan input varsa mevcut likiditede swap et
    if remaining_input > 1e-12 && active_liq > 0.0 {
        let extra_output = if token0_is_weth {
            single_tick_swap_token0_to_token1(sqrt_p, active_liq, remaining_input)
        } else {
            single_tick_swap_token1_to_token0(sqrt_p, active_liq, remaining_input)
        };
        total_output += extra_output;
    }

    // USDC dönüşümü
    let total_usdc = if token0_is_weth {
        total_output / USDC_DECIMALS
    } else {
        total_output / USDC_DECIMALS
    };

    let _eff_price = if amount_in_effective > 0.0 {
        total_usdc / (amount_in_effective / WETH_DECIMALS * WETH_DECIMALS / WETH_DECIMALS)
    } else {
        0.0
    };

    // Doğru efektif fiyat hesabı
    let total_weth_input = amount_in_effective;
    let real_eff_price = if total_weth_input > 0.0 { total_usdc / total_weth_input } else { 0.0 };

    MultiTickSwapResult {
        total_output: sanitize_f64(total_usdc),
        effective_price: sanitize_f64(real_eff_price),
        tick_crossings: crossings,
        final_tick: curr_tick,
        final_sqrt_price_x96: sqrt_p * Q96,
        final_liquidity: active_liq,
        used_real_bitmap: true,
    }
}

/// USDC → WETH gerçek multi-tick swap.
fn real_multitick_swap_usdc_to_weth(
    sqrt_price_f64: f64,
    liquidity: f64,
    current_tick: i32,
    amount_in_effective: f64,
    token0_is_weth: bool,
    tick_spacing: i32,
    bitmap: &TickBitmapData,
) -> MultiTickSwapResult {
    // USDC girişi: human-readable (ör: 10000.0) → raw (10000e6)
    let mut remaining_input = amount_in_effective * USDC_DECIMALS;
    let mut total_output = 0.0;
    let mut sqrt_p = sqrt_price_f64 / Q96;
    let mut active_liq = liquidity;
    let mut curr_tick = current_tick;
    let mut crossings: Vec<TickCrossing> = Vec::new();

    // USDC girdi yönü: token0=WETH → USDC token1 girdi → sağa git
    //                   token0=USDC → USDC token0 girdi → sola git
    let go_left = !token0_is_weth;

    let ordered_ticks = if go_left {
        let mut t: Vec<_> = bitmap.ticks.iter()
            .filter(|(&tick, info)| tick <= curr_tick && info.initialized)
            .map(|(&tick, info)| (tick, *info))
            .collect();
        t.sort_by(|a, b| b.0.cmp(&a.0));
        t
    } else {
        let mut t: Vec<_> = bitmap.ticks.iter()
            .filter(|(&tick, info)| tick > curr_tick && info.initialized)
            .map(|(&tick, info)| (tick, *info))
            .collect();
        t.sort_by_key(|(tick, _)| *tick);
        t
    };

    let max_crossings = 50;
    let mut crossings_count = 0;

    for (next_tick, tick_info) in &ordered_ticks {
        if remaining_input <= 1e-12 || crossings_count >= max_crossings {
            break;
        }

        let next_sqrt_p = tick_to_sqrt_price_x96(*next_tick) / Q96;

        let (input_consumed, output_produced, new_sqrt_p) = if token0_is_weth {
            // USDC = token1 girdi → sqrtPrice artar
            compute_swap_within_tick_token1(sqrt_p, next_sqrt_p, active_liq, remaining_input)
        } else {
            // USDC = token0 girdi → sqrtPrice azalır
            compute_swap_within_tick_token0(sqrt_p, next_sqrt_p, active_liq, remaining_input)
        };

        if input_consumed > 0.0 {
            total_output += output_produced;
            remaining_input -= input_consumed;
            sqrt_p = new_sqrt_p;

            crossings.push(TickCrossing {
                tick: *next_tick,
                liquidity: active_liq,
                input_consumed,
                output_produced,
                liquidity_net: tick_info.liquidity_net,
            });

            if (remaining_input > 1e-12) && ((new_sqrt_p - next_sqrt_p).abs() < 1e-30) {
                if go_left {
                    active_liq -= tick_info.liquidity_net as f64;
                } else {
                    active_liq += tick_info.liquidity_net as f64;
                }
                active_liq = active_liq.max(0.0);
                curr_tick = *next_tick;
                crossings_count += 1;
            }
        }
    }

    if remaining_input > 1e-12 && active_liq > 0.0 {
        let extra_output = if token0_is_weth {
            // USDC(token1) girdi → WETH(token0) çıktı
            single_tick_swap_token1_to_token0(sqrt_p, active_liq, remaining_input)
        } else {
            // USDC(token0) girdi → WETH(token1) çıktı
            single_tick_swap_token0_to_token1(sqrt_p, active_liq, remaining_input)
        };
        total_output += extra_output;
    }

    let total_weth = if token0_is_weth {
        total_output / WETH_DECIMALS
    } else {
        total_output / WETH_DECIMALS
    };

    let real_eff_price = if total_weth > 0.0 { amount_in_effective / total_weth } else { 0.0 };

    let _ = tick_spacing; // Kullanılmıyor ama API tutarlılığı için

    MultiTickSwapResult {
        total_output: sanitize_f64(total_weth),
        effective_price: sanitize_f64(real_eff_price),
        tick_crossings: crossings,
        final_tick: curr_tick,
        final_sqrt_price_x96: sqrt_p * Q96,
        final_liquidity: active_liq,
        used_real_bitmap: true,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tick-İçi Swap Hesaplama Yardımcıları
// ─────────────────────────────────────────────────────────────────────────────

/// Bir tick aralığı içinde token0 (x) girdi ile swap.
/// √P_new = L × √P / (L + Δx_raw × √P)   (sqrtPrice azalır)
/// Δy_raw = L × (√P - √P_new)              (token1 çıktı)
///
/// Returns: (input_consumed_raw, output_produced_raw, new_sqrt_price)
fn compute_swap_within_tick_token0(
    sqrt_p: f64,
    target_sqrt_p: f64,
    liquidity: f64,
    max_input_raw: f64,
) -> (f64, f64, f64) {
    if liquidity <= 0.0 || sqrt_p <= 0.0 {
        return (0.0, 0.0, sqrt_p);
    }

    // Bu aralıktaki maksimum tüketilebilir input (target_sqrt_p'ye kadar)
    // Δx_raw = L × (1/√P_target - 1/√P)
    let max_input_for_range = if target_sqrt_p > 0.0 && target_sqrt_p < sqrt_p {
        liquidity * (1.0 / target_sqrt_p - 1.0 / sqrt_p)
    } else {
        f64::MAX
    };

    let actual_input = max_input_raw.min(max_input_for_range.max(0.0));
    if actual_input <= 0.0 {
        return (0.0, 0.0, sqrt_p);
    }

    // Yeni sqrtPrice
    let denom = liquidity + actual_input * sqrt_p;
    if denom <= 0.0 {
        return (0.0, 0.0, sqrt_p);
    }
    let new_sqrt_p = liquidity * sqrt_p / denom;

    // Çıktı
    let output = liquidity * (sqrt_p - new_sqrt_p);

    (actual_input, output.abs(), new_sqrt_p)
}

/// Bir tick aralığı içinde token1 (y) girdi ile swap.
/// √P_new = √P + Δy_raw / L               (sqrtPrice artar)
/// Δx_raw = L × (1/√P - 1/√P_new)          (token0 çıktı)
fn compute_swap_within_tick_token1(
    sqrt_p: f64,
    target_sqrt_p: f64,
    liquidity: f64,
    max_input_raw: f64,
) -> (f64, f64, f64) {
    if liquidity <= 0.0 || sqrt_p <= 0.0 {
        return (0.0, 0.0, sqrt_p);
    }

    // Bu aralıktaki maksimum tüketilebilir input
    // Δy_raw = L × (√P_target - √P)
    let max_input_for_range = if target_sqrt_p > sqrt_p {
        liquidity * (target_sqrt_p - sqrt_p)
    } else {
        f64::MAX
    };

    let actual_input = max_input_raw.min(max_input_for_range.max(0.0));
    if actual_input <= 0.0 {
        return (0.0, 0.0, sqrt_p);
    }

    let new_sqrt_p = sqrt_p + actual_input / liquidity;
    if new_sqrt_p <= 0.0 {
        return (0.0, 0.0, sqrt_p);
    }

    let output = liquidity * (1.0 / sqrt_p - 1.0 / new_sqrt_p);

    (actual_input, output.abs(), new_sqrt_p)
}

/// Tek tick aralığında token0 → token1 swap (sınır yok)
fn single_tick_swap_token0_to_token1(sqrt_p: f64, liquidity: f64, input_raw: f64) -> f64 {
    if liquidity <= 0.0 || sqrt_p <= 0.0 || input_raw <= 0.0 {
        return 0.0;
    }
    let denom = liquidity + input_raw * sqrt_p;
    if denom <= 0.0 { return 0.0; }
    let sp_new = liquidity * sqrt_p / denom;
    (liquidity * (sqrt_p - sp_new)).abs()
}

/// Tek tick aralığında token1 → token0 swap (sınır yok)
fn single_tick_swap_token1_to_token0(sqrt_p: f64, liquidity: f64, input_raw: f64) -> f64 {
    if liquidity <= 0.0 || sqrt_p <= 0.0 || input_raw <= 0.0 {
        return 0.0;
    }
    let sp_new = sqrt_p + input_raw / liquidity;
    if sp_new <= 0.0 { return 0.0; }
    (liquidity * (1.0 / sqrt_p - 1.0 / sp_new)).abs()
}

// ─────────────────────────────────────────────────────────────────────────────
// FALLBACK: Dampening-Tabanlı Swap (TickBitmap yoksa)
// ─────────────────────────────────────────────────────────────────────────────

/// WETH → USDC dampening-tabanlı swap (geriye uyumlu v5.1)
fn dampened_swap_weth_to_usdc(
    sqrt_price_f64: f64,
    liquidity: f64,
    current_tick: i32,
    effective: f64,
    token0_is_weth: bool,
    tick_spacing: i32,
) -> f64 {
    let sqrt_price = sqrt_price_f64 / Q96;

    let (new_sqrt_price, raw_out) = if token0_is_weth {
        let amount_in_raw = effective * WETH_DECIMALS;
        let denom = liquidity + amount_in_raw * sqrt_price;
        if denom <= 0.0 { return 0.0; }
        let sp_new = liquidity * sqrt_price / denom;
        let out_raw = liquidity * (sqrt_price - sp_new);
        (sp_new, out_raw / USDC_DECIMALS)
    } else {
        let amount_in_raw = effective * WETH_DECIMALS;
        let sp_new = sqrt_price + amount_in_raw / liquidity;
        if sp_new <= 0.0 { return 0.0; }
        let out_raw = liquidity * (1.0 / sqrt_price - 1.0 / sp_new);
        (sp_new, out_raw / USDC_DECIMALS)
    };

    // NaN/Infinity güvenlik kontrolü
    if raw_out.is_nan() || raw_out.is_infinite() {
        return 0.0;
    }

    let new_sqrt_price_x96 = new_sqrt_price * Q96;
    let new_tick = sqrt_price_x96_to_tick(new_sqrt_price_x96);
    let ts = tick_spacing.max(1) as f64;
    let ticks_crossed = ((new_tick - current_tick).abs() as f64 / ts).ceil() as i32;
    let dampening = TICK_CROSS_DAMPENING.powi(ticks_crossed);

    let sonuc = raw_out.abs() * dampening;
    if sonuc.is_nan() || sonuc.is_infinite() { 0.0 } else { sonuc }
}

/// USDC → WETH dampening-tabanlı swap (geriye uyumlu v5.1)
fn dampened_swap_usdc_to_weth(
    sqrt_price_f64: f64,
    liquidity: f64,
    current_tick: i32,
    effective: f64,
    token0_is_weth: bool,
    tick_spacing: i32,
) -> f64 {
    let sqrt_price = sqrt_price_f64 / Q96;

    let (new_sqrt_price, raw_out) = if token0_is_weth {
        let amount_in_raw = effective * USDC_DECIMALS;
        let sp_new = sqrt_price + amount_in_raw / liquidity;
        if sp_new <= 0.0 { return 0.0; }
        let out_raw = liquidity * (1.0 / sqrt_price - 1.0 / sp_new);
        (sp_new, out_raw / WETH_DECIMALS)
    } else {
        let amount_in_raw = effective * USDC_DECIMALS;
        let denom = liquidity + amount_in_raw * sqrt_price;
        if denom <= 0.0 { return 0.0; }
        let sp_new = liquidity * sqrt_price / denom;
        let out_raw = liquidity * (sqrt_price - sp_new);
        (sp_new, out_raw / WETH_DECIMALS)
    };

    // NaN/Infinity güvenlik kontrolü
    if raw_out.is_nan() || raw_out.is_infinite() {
        return 0.0;
    }

    let new_sqrt_price_x96 = new_sqrt_price * Q96;
    let new_tick = sqrt_price_x96_to_tick(new_sqrt_price_x96);
    let ts = tick_spacing.max(1) as f64;
    let ticks_crossed = ((new_tick - current_tick).abs() as f64 / ts).ceil() as i32;
    let dampening = TICK_CROSS_DAMPENING.powi(ticks_crossed);

    let sonuc = raw_out.abs() * dampening;
    if sonuc.is_nan() || sonuc.is_infinite() { 0.0 } else { sonuc }
}

// ─────────────────────────────────────────────────────────────────────────────
// Uyumluluk Katmanı — Eski API (swap_weth_to_usdc / swap_usdc_to_weth)
// ─────────────────────────────────────────────────────────────────────────────

/// WETH → USDC swap (geriye uyumlu, TickBitmap opsiyonel).
/// TickBitmap verilmezse dampening moduna düşer.
#[allow(dead_code)]
pub fn swap_weth_to_usdc(
    sqrt_price_f64: f64,
    liquidity: f64,
    current_tick: i32,
    amount_in_weth: f64,
    fee_fraction: f64,
    token0_is_weth: bool,
    tick_spacing: i32,
) -> f64 {
    let result = swap_weth_to_usdc_multitick(
        sqrt_price_f64, liquidity, current_tick,
        amount_in_weth, fee_fraction, token0_is_weth,
        tick_spacing, None,
    );
    result.total_output
}

/// USDC → WETH swap (geriye uyumlu, TickBitmap opsiyonel).
#[allow(dead_code)]
pub fn swap_usdc_to_weth(
    sqrt_price_f64: f64,
    liquidity: f64,
    current_tick: i32,
    amount_in_usdc: f64,
    fee_fraction: f64,
    token0_is_weth: bool,
    tick_spacing: i32,
) -> f64 {
    let result = swap_usdc_to_weth_multitick(
        sqrt_price_f64, liquidity, current_tick,
        amount_in_usdc, fee_fraction, token0_is_weth,
        tick_spacing, None,
    );
    result.total_output
}

// ─────────────────────────────────────────────────────────────────────────────
// Likidite-Tabanlı Üst Sınır Hesaplama
// ─────────────────────────────────────────────────────────────────────────────

/// Havuzun aktif likiditesine göre güvenli maksimum swap miktarını hesapla.
///
/// v7.0: f64 imzası geriye uyumluluk için korunuyor ama
/// yeni çağrılar `exact::max_safe_swap_amount_u256` kullanmalıdır.
#[allow(dead_code)]
pub fn max_safe_swap_amount(
    sqrt_price_f64: f64,
    liquidity: f64,
    token0_is_weth: bool,
    current_tick: i32,
    tick_spacing: i32,
) -> f64 {
    if sqrt_price_f64 <= 0.0 || liquidity <= 0.0
       || sqrt_price_f64.is_nan() || sqrt_price_f64.is_infinite()
       || liquidity.is_nan() || liquidity.is_infinite()
    {
        return 0.0;
    }
    // f64 değerlerden U256 yaklaşık dönüşüm
    let sqrt_price_u256 = alloy::primitives::U256::from(
        crate::types::safe_f64_to_u128(sqrt_price_f64)
    );
    let liquidity_u128 = if liquidity >= u128::MAX as f64 {
        u128::MAX
    } else if liquidity < 0.0 {
        0u128
    } else {
        liquidity as u128
    };
    let result = exact::max_safe_swap_amount_u256(sqrt_price_u256, liquidity_u128, token0_is_weth, current_tick, tick_spacing);
    sanitize_f64(result)
}

// ─────────────────────────────────────────────────────────────────────────────
// Arbitraj Kâr Hesaplama — Multi-Tick Aware
// ─────────────────────────────────────────────────────────────────────────────

/// İki havuz arasında arbitraj kârını hesapla.
/// TickBitmap verilirse gerçek multi-tick, yoksa dampening kullanır.
#[allow(dead_code)]
pub fn compute_arbitrage_profit(
    amount_in_weth: f64,
    sell_pool: &PoolState,
    sell_fee_fraction: f64,
    buy_pool: &PoolState,
    buy_fee_fraction: f64,
    gas_cost_usd: f64,
    flash_loan_fee_bps: f64,
    eth_price_usd: f64,
    sell_token0_is_weth: bool,
    sell_tick_spacing: i32,
    buy_tick_spacing: i32,
    buy_token0_is_weth: bool,
) -> f64 {
    compute_arbitrage_profit_with_bitmap(
        amount_in_weth,
        sell_pool, sell_fee_fraction,
        buy_pool, buy_fee_fraction,
        gas_cost_usd, flash_loan_fee_bps, eth_price_usd,
        sell_token0_is_weth, sell_tick_spacing, buy_tick_spacing,
        None, None,
        buy_token0_is_weth,
    )
}

/// İki havuz arasında arbitraj kârını hesapla (TickBitmap destekli).
///
/// v7.0 (U256 Stabilizasyon): Tüm swap hesaplamaları artık
/// `exact::compute_exact_swap` üzerinden yapılır — Uniswap V3'ün
/// FullMath/SqrtPriceMath/SwapMath kütüphanelerinin birebir U256 portu.
///
/// Akış:
///   1. f64 WETH miktarı → U256 wei dönüşümü
///   2. Pahalı havuzda exact U256 swap (WETH → USDC)
///   3. Ucuz havuzda exact U256 swap (USDC → WETH)
///   4. Flash loan geri ödeme (U256)
///   5. Net kâr → f64 USD (optimizer'a geri dönüş)
///
/// Bu sayede off-chain hesaplama, on-chain sonuçla wei bazında eşleşir.
pub fn compute_arbitrage_profit_with_bitmap(
    amount_in_weth: f64,
    sell_pool: &PoolState,
    sell_fee_fraction: f64,
    buy_pool: &PoolState,
    buy_fee_fraction: f64,
    gas_cost_usd: f64,
    flash_loan_fee_bps: f64,
    eth_price_usd: f64,
    sell_token0_is_weth: bool,
    sell_tick_spacing: i32,
    buy_tick_spacing: i32,
    sell_bitmap: Option<&TickBitmapData>,
    buy_bitmap: Option<&TickBitmapData>,
    buy_token0_is_weth: bool,
) -> f64 {
    if amount_in_weth <= 0.0 {
        return f64::NEG_INFINITY;
    }

    // ═══ U256 EXACT MATH — On-Chain Deterministik Kesinlik ═══

    // f64 → U256 wei dönüşümü
    let amount_in_wei = alloy::primitives::U256::from(
        crate::types::safe_f64_to_u128(amount_in_weth * 1e18)
    );
    if amount_in_wei.is_zero() {
        return f64::NEG_INFINITY;
    }

    // Fee fraction → pips (1e6 bazında: 0.0005 → 500)
    let sell_fee_pips = exact::fee_fraction_to_pips(sell_fee_fraction);
    let buy_fee_pips = exact::fee_fraction_to_pips(buy_fee_fraction);

    // 1. WETH'i pahalı havuzda sat → USDC al (exact U256 swap)
    // v20.0: Her havuzun kendi token0_is_weth değeri kullanılır
    let sell_zero_for_one = sell_token0_is_weth;
    let sell_result = exact::compute_exact_swap(
        sell_pool.sqrt_price_x96,
        sell_pool.liquidity,
        sell_pool.tick,
        amount_in_wei,
        sell_zero_for_one,
        sell_fee_pips,
        sell_tick_spacing,
        sell_bitmap,
    );

    if sell_result.amount_out.is_zero() {
        return f64::NEG_INFINITY;
    }

    // 2. USDC → WETH geri al (exact U256 swap)
    // v20.0: buy pool'un kendi token sıralaması kullanılır
    let buy_zero_for_one = !buy_token0_is_weth;
    let buy_result = exact::compute_exact_swap(
        buy_pool.sqrt_price_x96,
        buy_pool.liquidity,
        buy_pool.tick,
        sell_result.amount_out,
        buy_zero_for_one,
        buy_fee_pips,
        buy_tick_spacing,
        buy_bitmap,
    );

    if buy_result.amount_out.is_zero() {
        return f64::NEG_INFINITY;
    }

    // 3. Flash loan geri ödeme (U256 hassasiyetinde)
    let flash_loan_fee_rate = flash_loan_fee_bps / 10_000.0;
    let flash_loan_fee_wei = alloy::primitives::U256::from(
        crate::types::safe_f64_to_u128(amount_in_weth * flash_loan_fee_rate * 1e18)
    );
    let repay_amount = amount_in_wei + flash_loan_fee_wei;

    // 4. Net kâr → USD (optimizer için f64'e geri dönüş)
    if buy_result.amount_out > repay_amount {
        let profit_wei = buy_result.amount_out - repay_amount;
        let profit_weth = exact::u256_to_f64(profit_wei) / 1e18;
        profit_weth * eth_price_usd - gas_cost_usd
    } else {
        let loss_wei = repay_amount - buy_result.amount_out;
        let loss_weth = exact::u256_to_f64(loss_wei) / 1e18;
        -(loss_weth * eth_price_usd) - gas_cost_usd
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Newton-Raphson Türev Hesaplayıcı
// ─────────────────────────────────────────────────────────────────────────────

fn profit_derivative(
    amount_in_weth: f64,
    sell_pool: &PoolState,
    sell_fee: f64,
    buy_pool: &PoolState,
    buy_fee: f64,
    gas_cost_usd: f64,
    flash_loan_fee_bps: f64,
    eth_price_usd: f64,
    sell_token0_is_weth: bool,
    sell_ts: i32,
    buy_ts: i32,
    sell_bitmap: Option<&TickBitmapData>,
    buy_bitmap: Option<&TickBitmapData>,
    buy_token0_is_weth: bool,
) -> f64 {
    let h = (amount_in_weth * 1e-7).max(1e-10);

    let f_plus = compute_arbitrage_profit_with_bitmap(
        amount_in_weth + h,
        sell_pool, sell_fee, buy_pool, buy_fee,
        gas_cost_usd, flash_loan_fee_bps, eth_price_usd,
        sell_token0_is_weth, sell_ts, buy_ts,
        sell_bitmap, buy_bitmap,
        buy_token0_is_weth,
    );
    let f_minus = compute_arbitrage_profit_with_bitmap(
        amount_in_weth - h,
        sell_pool, sell_fee, buy_pool, buy_fee,
        gas_cost_usd, flash_loan_fee_bps, eth_price_usd,
        sell_token0_is_weth, sell_ts, buy_ts,
        sell_bitmap, buy_bitmap,
        buy_token0_is_weth,
    );

    (f_plus - f_minus) / (2.0 * h)
}

fn profit_second_derivative(
    amount_in_weth: f64,
    sell_pool: &PoolState,
    sell_fee: f64,
    buy_pool: &PoolState,
    buy_fee: f64,
    gas_cost_usd: f64,
    flash_loan_fee_bps: f64,
    eth_price_usd: f64,
    sell_token0_is_weth: bool,
    sell_ts: i32,
    buy_ts: i32,
    sell_bitmap: Option<&TickBitmapData>,
    buy_bitmap: Option<&TickBitmapData>,
    buy_token0_is_weth: bool,
) -> f64 {
    let h = (amount_in_weth * 1e-5).max(1e-8);

    let fp_plus = profit_derivative(
        amount_in_weth + h,
        sell_pool, sell_fee, buy_pool, buy_fee,
        gas_cost_usd, flash_loan_fee_bps, eth_price_usd,
        sell_token0_is_weth, sell_ts, buy_ts,
        sell_bitmap, buy_bitmap,
        buy_token0_is_weth,
    );
    let fp_minus = profit_derivative(
        amount_in_weth - h,
        sell_pool, sell_fee, buy_pool, buy_fee,
        gas_cost_usd, flash_loan_fee_bps, eth_price_usd,
        sell_token0_is_weth, sell_ts, buy_ts,
        sell_bitmap, buy_bitmap,
        buy_token0_is_weth,
    );

    (fp_plus - fp_minus) / (2.0 * h)
}

// ─────────────────────────────────────────────────────────────────────────────
// Newton-Raphson Optimizasyonu — TickBitmap-Aware
// ─────────────────────────────────────────────────────────────────────────────

/// Newton-Raphson sonucu
#[derive(Debug, Clone)]
pub struct OptimalAmountResult {
    pub optimal_amount: f64,
    pub expected_profit: f64,
    pub converged: bool,
    pub iterations: u32,
}

/// Newton-Raphson ile optimal flash loan miktarını bul.
/// TickBitmap verilirse multi-tick hassasiyetinde çalışır.
#[allow(dead_code)]
pub fn find_optimal_amount(
    sell_pool: &PoolState,
    sell_fee: f64,
    buy_pool: &PoolState,
    buy_fee: f64,
    gas_cost_usd: f64,
    flash_loan_fee_bps: f64,
    eth_price_usd: f64,
    max_amount_weth: f64,
    sell_token0_is_weth: bool,
    sell_tick_spacing: i32,
    buy_tick_spacing: i32,
    buy_token0_is_weth: bool,
) -> OptimalAmountResult {
    find_optimal_amount_with_bitmap(
        sell_pool, sell_fee, buy_pool, buy_fee,
        gas_cost_usd, flash_loan_fee_bps, eth_price_usd,
        max_amount_weth, sell_token0_is_weth,
        sell_tick_spacing, buy_tick_spacing,
        None, None,
        buy_token0_is_weth,
    )
}

/// Newton-Raphson (TickBitmap destekli + Hard Liquidity Cap).
/// v20.0: sell ve buy havuzları farklı token sıralamasına sahip olabilir.
///        Her havuzun kendi token0_is_weth değeri bağımsız kullanılır.
pub fn find_optimal_amount_with_bitmap(
    sell_pool: &PoolState,
    sell_fee: f64,
    buy_pool: &PoolState,
    buy_fee: f64,
    gas_cost_usd: f64,
    flash_loan_fee_bps: f64,
    eth_price_usd: f64,
    max_amount_weth: f64,
    sell_token0_is_weth: bool,
    sell_tick_spacing: i32,
    buy_tick_spacing: i32,
    sell_bitmap: Option<&TickBitmapData>,
    buy_bitmap: Option<&TickBitmapData>,
    buy_token0_is_weth: bool,
) -> OptimalAmountResult {
    let max_iterations: u32 = 50;
    let tolerance = 1e-8;
    let min_amount = 0.0001;

    // ── Hard Liquidity Cap (v11.0 + v20.0 decimal normalization) ─
    // v20.0: Her havuzun kendi token0_is_weth değeri kullanılır.
    // Farklı token sıralamasına sahip havuzlardan (ör: WETH/USDC vs USDC/WETH)
    // doğru yönde likidite kapasitesi hesaplanır.
    let hard_cap_sell = exact::hard_liquidity_cap_weth(
        sell_pool.sqrt_price_x96,
        sell_pool.liquidity,
        sell_pool.tick,
        sell_token0_is_weth,
        sell_bitmap,
        sell_tick_spacing,
    );
    let hard_cap_buy = exact::hard_liquidity_cap_weth(
        buy_pool.sqrt_price_x96,
        buy_pool.liquidity,
        buy_pool.tick,
        buy_token0_is_weth,
        buy_bitmap,
        buy_tick_spacing,
    );

    // Eski single-tick cap (geriye uyumluluk + karşılaştırma)
    let liq_cap_sell = exact::max_safe_swap_amount_u256(
        sell_pool.sqrt_price_x96, sell_pool.liquidity, sell_token0_is_weth,
        sell_pool.tick, sell_tick_spacing,
    );
    let liq_cap_buy = exact::max_safe_swap_amount_u256(
        buy_pool.sqrt_price_x96, buy_pool.liquidity, buy_token0_is_weth,
        buy_pool.tick, buy_tick_spacing,
    );

    // v16.0: Hard cap ve single-tick cap'in minimumunu al.
    // Eski: `* 2.0` çarpanı single-tick kapasiteyi yapay olarak şişiriyor
    // ve NR'nin havuzda olmayan likiditeyi hedeflemesine yol açıyordu.
    // Yeni: Her iki metriğin minimumunu al, %99.9 güvenlik marjı zaten
    // hard_liquidity_cap_weth içinde uygulanıyor.
    let sell_cap = hard_cap_sell.min(liq_cap_sell.max(0.001)).max(0.001);
    let buy_cap = hard_cap_buy.min(liq_cap_buy.max(0.001)).max(0.001);
    let effective_max = max_amount_weth
        .min(sell_cap)
        .min(buy_cap);

    eprintln!(
        "     \u{1f4ca} [Liquidity Cap] sell_hard={:.4} buy_hard={:.4} sell_single={:.4} buy_single={:.4} → effective_max={:.4} WETH",
        hard_cap_sell, hard_cap_buy, liq_cap_sell, liq_cap_buy, effective_max,
    );

    if effective_max <= min_amount {
        return OptimalAmountResult {
            optimal_amount: 0.0,
            expected_profit: 0.0,
            converged: false,
            iterations: 0,
        };
    }

    // ── AŞAMA 1: Hibrit Kaba Tarama ──────────────────────────────
    // v22.0: 40 → 25 adım. Quadratic spacing küçük miktarlarda daha yoğun
    // tarama yapar, büyük miktarlarda seyrekleşir. 25 adım yeterli çözünürlük
    // sağlar, 15 iterasyon (~0.5ms) tasarruf eder.
    let mut best_amount = 0.0;
    let mut best_profit = f64::NEG_INFINITY;
    let scan_steps = 25;

    for i in 1..=scan_steps {
        let fraction = i as f64 / scan_steps as f64;
        let amount = min_amount + (effective_max - min_amount) * fraction * fraction;

        let profit = compute_arbitrage_profit_with_bitmap(
            amount,
            sell_pool, sell_fee, buy_pool, buy_fee,
            gas_cost_usd, flash_loan_fee_bps, eth_price_usd,
            sell_token0_is_weth, sell_tick_spacing, buy_tick_spacing,
            sell_bitmap, buy_bitmap,
            buy_token0_is_weth,
        );

        if profit > best_profit {
            best_profit = profit;
            best_amount = amount;
        }
    }

    if best_profit <= f64::NEG_INFINITY + 1.0 || best_amount <= 0.0 {
        return OptimalAmountResult {
            optimal_amount: 0.0,
            expected_profit: best_profit.max(0.0),
            converged: false,
            iterations: 0,
        };
    }

    // ── AŞAMA 2: Newton-Raphson İnce Ayar ────────────────────────
    let mut x = best_amount;
    let mut converged = false;
    let mut final_iterations: u32 = 0;

    for i in 0..max_iterations {
        final_iterations = i + 1;

        let f_prime = profit_derivative(
            x, sell_pool, sell_fee, buy_pool, buy_fee,
            gas_cost_usd, flash_loan_fee_bps, eth_price_usd,
            sell_token0_is_weth, sell_tick_spacing, buy_tick_spacing,
            sell_bitmap, buy_bitmap,
            buy_token0_is_weth,
        );

        let f_double_prime = profit_second_derivative(
            x, sell_pool, sell_fee, buy_pool, buy_fee,
            gas_cost_usd, flash_loan_fee_bps, eth_price_usd,
            sell_token0_is_weth, sell_tick_spacing, buy_tick_spacing,
            sell_bitmap, buy_bitmap,
            buy_token0_is_weth,
        );

        if f_double_prime.abs() < 1e-20 {
            break;
        }

        let step = f_prime / f_double_prime;
        let mut x_new = x - step;

        if (x_new - x).abs() > effective_max * 0.5 {
            x_new = x - step * 0.25;
        }

        x_new = x_new.clamp(min_amount, effective_max);

        if (x_new - x).abs() < tolerance {
            converged = true;
            x = x_new;
            break;
        }

        x = x_new;
    }

    // v16.0: NR yakınsama sonrası nihai güvenlik tavanı.
    // NR iterasyonları sırasında clamp uygulanıyor ama yakınsama sonrası
    // son bir kez daha effective_max ile sınırla — havuz kapasitesinin
    // %99.9'unu (slippage payı) ASLA aşamaz.
    x = x.clamp(min_amount, effective_max);

    let final_profit = compute_arbitrage_profit_with_bitmap(
        x, sell_pool, sell_fee, buy_pool, buy_fee,
        gas_cost_usd, flash_loan_fee_bps, eth_price_usd,
        sell_token0_is_weth, sell_tick_spacing, buy_tick_spacing,
        sell_bitmap, buy_bitmap,
        buy_token0_is_weth,
    );

    OptimalAmountResult {
        optimal_amount: x,
        expected_profit: final_profit,
        converged,
        iterations: final_iterations,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Testler
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TickInfo;
    use std::collections::HashMap;
    use std::time::Instant;
    use proptest::prelude::*;

    /// Test havuz durumu oluştur (Base Network gerçekçi değerler).
    /// v7.0: sqrt_price_x96 artık doğru U256 olarak hesaplanır (exact::get_sqrt_ratio_at_tick).
    fn make_test_pool(eth_price: f64) -> PoolState {
        let price_ratio = eth_price * 1e-12;
        let sqrt_price = price_ratio.sqrt();
        let sqrt_price_x96 = sqrt_price * Q96;
        let tick = (price_ratio.ln() / LOG_TICK_BASE).floor() as i32;
        let liquidity: u128 = 50_000_000_000_000_000_000; // 5e19

        // U256 sqrtPriceX96'yı tick'ten exact hesapla (deterministik)
        let sqrt_price_x96_u256 = crate::math::exact::get_sqrt_ratio_at_tick(tick);

        PoolState {
            sqrt_price_x96: sqrt_price_x96_u256,
            sqrt_price_f64: sqrt_price_x96,
            tick,
            liquidity,
            liquidity_f64: liquidity as f64,
            eth_price_usd: eth_price,
            last_block: 0,
            last_update: Instant::now(),
            is_initialized: true,
            bytecode: None,
            tick_bitmap: None,
            live_fee_bps: None,
        }
    }

    /// Test TickBitmap oluştur (mevcut tick etrafında birkaç başlatılmış tick)
    fn make_test_bitmap(current_tick: i32, tick_spacing: i32) -> TickBitmapData {
        let mut ticks = HashMap::new();

        // Mevcut tick'in etrafında 10 tick sınırı oluştur
        for i in -5..=5 {
            let tick = ((current_tick / tick_spacing) + i) * tick_spacing;
            let liq_net = if i < 0 {
                // Sol tick'ler: yaklaştıkça likidite artar
                5_000_000_000_000_000_000i128 // 5e18
            } else if i > 0 {
                // Sağ tick'ler: uzaklaştıkça likidite azalır
                -5_000_000_000_000_000_000i128
            } else {
                0i128
            };

            ticks.insert(tick, TickInfo {
                liquidity_gross: liq_net.unsigned_abs(),
                liquidity_net: liq_net,
                initialized: true,
            });
        }

        TickBitmapData {
            words: HashMap::new(),
            ticks,
            snapshot_block: 0,
            sync_duration_us: 0,
            scan_range: 500,
        }
    }

    #[test]
    fn test_tick_price_roundtrip() {
        let tick = -200287i32;
        let price = tick_to_price_ratio(tick);
        let sqrt_x96 = tick_to_sqrt_price_x96(tick);
        let recovered_tick = sqrt_price_x96_to_tick(sqrt_x96);
        assert!(
            (recovered_tick - tick).abs() <= 1,
            "Tick roundtrip hatası: {} → {}", tick, recovered_tick
        );
        assert!(price > 1e-10 && price < 1e-7, "price_ratio={}", price);
    }

    #[test]
    fn test_compute_eth_price_token0_weth() {
        let pool = make_test_pool(2000.0);
        let price = compute_eth_price(
            pool.sqrt_price_f64, pool.tick, 18, 6, true,
        );
        assert!(
            (price - 2000.0).abs() < 5.0,
            "ETH fiyatı ~2000 olmalı, hesaplanan: {:.2}", price
        );
    }

    #[test]
    fn test_compute_eth_price_various() {
        for expected in [1500.0, 2000.0, 2500.0, 3000.0, 5000.0] {
            let pool = make_test_pool(expected);
            let price = compute_eth_price(
                pool.sqrt_price_f64, pool.tick, 18, 6, true,
            );
            let err_pct = ((price - expected) / expected).abs() * 100.0;
            assert!(
                err_pct < 0.1,
                "ETH={:.0}, hesaplanan={:.2}, hata={:.4}%", expected, price, err_pct
            );
        }
    }

    #[test]
    fn test_swap_weth_to_usdc_token0_weth() {
        let pool = make_test_pool(2000.0);
        let usdc_out = swap_weth_to_usdc(
            pool.sqrt_price_f64,
            pool.liquidity_f64,
            pool.tick,
            1.0,    // 1 WETH
            0.0005, // %0.05 fee
            true,   // token0=WETH
            10,     // tick_spacing
        );
        assert!(
            usdc_out > 1900.0 && usdc_out < 2100.0,
            "1 WETH ≈ 2000 USDC olmalı, hesaplanan: {:.2}", usdc_out
        );
    }

    #[test]
    fn test_swap_usdc_to_weth_token0_weth() {
        let pool = make_test_pool(2000.0);
        let weth_out = swap_usdc_to_weth(
            pool.sqrt_price_f64,
            pool.liquidity_f64,
            pool.tick,
            2000.0,  // 2000 USDC
            0.0005,  // %0.05 fee
            true,    // token0=WETH
            10,      // tick_spacing
        );
        assert!(
            weth_out > 0.90 && weth_out < 1.10,
            "2000 USDC ≈ 1 WETH olmalı, hesaplanan: {:.6}", weth_out
        );
    }

    #[test]
    fn test_large_swap_dampening() {
        let pool = make_test_pool(2000.0);
        let small_out = swap_weth_to_usdc(
            pool.sqrt_price_f64, pool.liquidity_f64, pool.tick,
            1.0, 0.0005, true, 10,
        );
        let large_out = swap_weth_to_usdc(
            pool.sqrt_price_f64, pool.liquidity_f64, pool.tick,
            20.0, 0.0005, true, 10,
        );
        let small_price = small_out / 1.0;
        let large_price = large_out / 20.0;
        assert!(
            large_price < small_price,
            "Büyük swap'ta fiyat kötüleşmeli: small={:.2}, large={:.2}",
            small_price, large_price
        );
    }

    #[test]
    fn test_max_safe_swap_amount() {
        let pool = make_test_pool(2000.0);
        let max_weth = max_safe_swap_amount(
            pool.sqrt_price_f64, pool.liquidity_f64, true,
            pool.tick, 10,
        );
        assert!(
            max_weth > 1.0 && max_weth < 1_000_000.0,
            "Güvenli maks swap makul olmalı: {:.4} WETH", max_weth
        );
    }

    #[test]
    fn test_newton_raphson_tick_aware() {
        let buy_pool = make_test_pool(1980.0);
        let sell_pool = make_test_pool(2020.0);

        let result = find_optimal_amount(
            &sell_pool, 0.0005,
            &buy_pool, 0.01,
            0.10,
            5.0,
            2000.0,
            10.0,
            true,
            10,
            10,
            true,
        );

        println!(
            "NR Sonuç: miktar={:.6} WETH, kâr={:.4}$, iter={}, yakın={}",
            result.optimal_amount, result.expected_profit,
            result.iterations, result.converged
        );

        assert!(result.expected_profit > 0.0, "Kâr pozitif olmalı");
        assert!(result.optimal_amount > 0.0, "Optimal miktar > 0 olmalı");
        assert!(result.optimal_amount <= 10.0, "Optimal miktar max'ı aşmamalı");
    }

    #[test]
    fn test_multitick_swap_with_bitmap() {
        let pool = make_test_pool(2000.0);
        let bitmap = make_test_bitmap(pool.tick, 10);

        // Multi-tick swap (TickBitmap ile)
        let result = swap_weth_to_usdc_multitick(
            pool.sqrt_price_f64,
            pool.liquidity_f64,
            pool.tick,
            5.0,    // 5 WETH
            0.0005, // %0.05 fee
            true,   // token0=WETH
            10,     // tick_spacing
            Some(&bitmap),
        );

        println!(
            "Multi-tick sonuç: çıktı={:.2} USDC, eff_price={:.2}, tick_geçişi={}, bitmap={}",
            result.total_output, result.effective_price,
            result.tick_crossings.len(), result.used_real_bitmap
        );

        // Bitmap kullanıldı mı?
        assert!(result.used_real_bitmap, "Gerçek TickBitmap kullanılmalı");
        // Çıktı makul mü?
        assert!(
            result.total_output > 9000.0 && result.total_output < 11000.0,
            "5 WETH ≈ 10000 USDC civarı olmalı, hesaplanan: {:.2}",
            result.total_output
        );

        // Dampening ile karşılaştır
        let dampened_out = swap_weth_to_usdc(
            pool.sqrt_price_f64, pool.liquidity_f64, pool.tick,
            5.0, 0.0005, true, 10,
        );
        println!(
            "Karşılaştırma: bitmap={:.2}, dampened={:.2}",
            result.total_output, dampened_out
        );
    }

    #[test]
    fn test_multitick_swap_tick_crossings_detail() {
        let pool = make_test_pool(2000.0);
        let bitmap = make_test_bitmap(pool.tick, 10);

        let result = swap_weth_to_usdc_multitick(
            pool.sqrt_price_f64,
            pool.liquidity_f64,
            pool.tick,
            10.0,   // 10 WETH (büyük miktar → daha fazla tick geçişi)
            0.0005,
            true,
            10,
            Some(&bitmap),
        );

        println!("10 WETH swap detayı:");
        for (i, crossing) in result.tick_crossings.iter().enumerate() {
            println!(
                "  Tick #{}: tick={}, liq={:.2e}, input={:.4e}, output={:.4e}, liqNet={}",
                i + 1, crossing.tick, crossing.liquidity,
                crossing.input_consumed, crossing.output_produced,
                crossing.liquidity_net
            );
        }
        println!(
            "  Toplam: {:.2} USDC, eff_price={:.2}, final_tick={}",
            result.total_output, result.effective_price, result.final_tick
        );

        assert!(result.total_output > 0.0, "Çıktı pozitif olmalı");
    }

    #[test]
    fn test_newton_raphson_with_bitmap() {
        let buy_pool = make_test_pool(1980.0);
        let sell_pool = make_test_pool(2020.0);
        let sell_bitmap = make_test_bitmap(sell_pool.tick, 10);
        let buy_bitmap = make_test_bitmap(buy_pool.tick, 10);

        let result = find_optimal_amount_with_bitmap(
            &sell_pool, 0.0005,
            &buy_pool, 0.01,
            0.10,
            5.0,
            2000.0,
            10.0,
            true,
            10,
            10,
            Some(&sell_bitmap),
            Some(&buy_bitmap),
            true,
        );

        println!(
            "NR+Bitmap: miktar={:.6} WETH, kâr={:.4}$, iter={}, yakın={}",
            result.optimal_amount, result.expected_profit,
            result.iterations, result.converged
        );

        assert!(result.expected_profit > 0.0, "Kâr pozitif olmalı");
        assert!(result.optimal_amount > 0.0, "Optimal miktar > 0 olmalı");
    }

    // ─────────────────────────────────────────────────────────────────────
    // PROPTEST — Çökme Dayanıklılık Testleri (Property-Based Stress Test)
    //
    // Amaç: math.rs motoruna milyonlarca rastgele ekstrem değer basarak
    // hiçbir koşulda panic!, NaN veya Infinity üretmediğini kanıtlamak.
    // ─────────────────────────────────────────────────────────────────────

    /// Yardımcı: Rastgele bir sqrtPriceX96 f64 değeri üret.
    /// Gerçek havuzlarda bu değer kabaca 1e18..1e30 arasındadır,
    /// ancak stres testi için 0.0 ve f64::MAX dahil tüm aralığı kapsıyoruz.
    fn arb_sqrt_price_x96() -> impl Strategy<Value = f64> {
        prop_oneof![
            // %60 — Gerçekçi aralık (Base ağındaki tipik değerler)
            6 => 1e18_f64..1e30_f64,
            // %20 — Sıfır ve sıfıra yakın kenar durumlar
            2 => prop::num::f64::ANY.prop_map(|v| v.abs().min(1.0)),
            // %10 — Aşırı büyük değerler (u256 aralığına yakın)
            1 => 1e30_f64..1e77_f64,
            // %10 — Negatif ve özel değerler (fonksiyonlar bunları sıfıra düşürmeli)
            1 => prop::num::f64::ANY.prop_map(|v| if v.is_nan() { 0.0 } else { v }),
        ]
    }

    /// Yardımcı: Rastgele bir likidite değeri üret.
    fn arb_liquidity() -> impl Strategy<Value = f64> {
        prop_oneof![
            // %50 — Gerçekçi aralık
            5 => 1e10_f64..1e25_f64,
            // %20 — Sıfır ve sıfıra yakın
            2 => 0.0_f64..1.0_f64,
            // %20 — Çok büyük likidite
            2 => 1e25_f64..1e38_f64,
            // %10 — Tam sıfır
            1 => Just(0.0_f64),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(10_000))]

        // ─── TEST 1: compute_eth_price asla NaN/Inf/panic üretmemeli ───
        #[test]
        fn stres_compute_eth_price(
            sqrt_price_x96 in arb_sqrt_price_x96(),
            tick in -887272..=887272i32,
            token0_is_weth in proptest::bool::ANY,
        ) {
            let sonuc = compute_eth_price(
                sqrt_price_x96,
                tick,
                18,  // WETH decimals
                6,   // USDC decimals
                token0_is_weth,
            );
            prop_assert!(!sonuc.is_nan(),
                "compute_eth_price NaN döndü! sqrt_price_x96={}, tick={}, t0_weth={}",
                sqrt_price_x96, tick, token0_is_weth);
            prop_assert!(!sonuc.is_infinite(),
                "compute_eth_price Infinity döndü! sqrt_price_x96={}, tick={}, t0_weth={}",
                sqrt_price_x96, tick, token0_is_weth);
            prop_assert!(sonuc >= 0.0,
                "compute_eth_price negatif döndü! sonuc={}, sqrt_price_x96={}, tick={}",
                sonuc, sqrt_price_x96, tick);
        }

        // ─── TEST 2: swap_weth_to_usdc_multitick asla çökmemeli ───────
        #[test]
        fn stres_swap_weth_to_usdc(
            sqrt_price_x96 in arb_sqrt_price_x96(),
            liquidity in arb_liquidity(),
            tick in -887272..=887272i32,
            amount_in in (0.0_f64..1e12_f64),
            fee in (0.0_f64..0.1_f64),
            token0_is_weth in proptest::bool::ANY,
            tick_spacing in prop::sample::select(vec![1i32, 10, 60, 200]),
        ) {
            let sonuc = swap_weth_to_usdc_multitick(
                sqrt_price_x96,
                liquidity,
                tick,
                amount_in,
                fee,
                token0_is_weth,
                tick_spacing,
                None, // TickBitmap yok → dampening fallback
            );
            prop_assert!(!sonuc.total_output.is_nan(),
                "swap_weth_to_usdc NaN! sqrtP={}, liq={}, tick={}, amt={}",
                sqrt_price_x96, liquidity, tick, amount_in);
            prop_assert!(!sonuc.total_output.is_infinite(),
                "swap_weth_to_usdc Infinity! sqrtP={}, liq={}, tick={}, amt={}",
                sqrt_price_x96, liquidity, tick, amount_in);
            prop_assert!(!sonuc.effective_price.is_nan(),
                "swap_weth_to_usdc eff_price NaN! sqrtP={}, liq={}, tick={}",
                sqrt_price_x96, liquidity, tick);
            prop_assert!(!sonuc.effective_price.is_infinite(),
                "swap_weth_to_usdc eff_price Infinity! sqrtP={}, liq={}, tick={}",
                sqrt_price_x96, liquidity, tick);
            prop_assert!(sonuc.total_output >= 0.0,
                "swap_weth_to_usdc negatif çıktı! output={}", sonuc.total_output);
        }

        // ─── TEST 3: swap_usdc_to_weth_multitick asla çökmemeli ───────
        #[test]
        fn stres_swap_usdc_to_weth(
            sqrt_price_x96 in arb_sqrt_price_x96(),
            liquidity in arb_liquidity(),
            tick in -887272..=887272i32,
            amount_in in (0.0_f64..1e12_f64),
            fee in (0.0_f64..0.1_f64),
            token0_is_weth in proptest::bool::ANY,
            tick_spacing in prop::sample::select(vec![1i32, 10, 60, 200]),
        ) {
            let sonuc = swap_usdc_to_weth_multitick(
                sqrt_price_x96,
                liquidity,
                tick,
                amount_in,
                fee,
                token0_is_weth,
                tick_spacing,
                None,
            );
            prop_assert!(!sonuc.total_output.is_nan(),
                "swap_usdc_to_weth NaN! sqrtP={}, liq={}, tick={}, amt={}",
                sqrt_price_x96, liquidity, tick, amount_in);
            prop_assert!(!sonuc.total_output.is_infinite(),
                "swap_usdc_to_weth Infinity! sqrtP={}, liq={}, tick={}, amt={}",
                sqrt_price_x96, liquidity, tick, amount_in);
            prop_assert!(!sonuc.effective_price.is_nan(),
                "swap_usdc_to_weth eff_price NaN! sqrtP={}, liq={}, tick={}",
                sqrt_price_x96, liquidity, tick);
            prop_assert!(!sonuc.effective_price.is_infinite(),
                "swap_usdc_to_weth eff_price Infinity! sqrtP={}, liq={}, tick={}",
                sqrt_price_x96, liquidity, tick);
            prop_assert!(sonuc.total_output >= 0.0,
                "swap_usdc_to_weth negatif çıktı! output={}", sonuc.total_output);
        }

        // ─── TEST 4: tick_to_price_ratio aşırı tick'lerde çökmemeli ───
        #[test]
        fn stres_tick_to_price_ratio(
            tick in -887272..=887272i32,
        ) {
            let sonuc = tick_to_price_ratio(tick);
            prop_assert!(!sonuc.is_nan(),
                "tick_to_price_ratio NaN! tick={}", tick);
            // Aşırı tick'lerde Infinity olabilir ama panic olmamalı
            prop_assert!(sonuc >= 0.0,
                "tick_to_price_ratio negatif! tick={}, sonuc={}", tick, sonuc);
        }

        // ─── TEST 5: sqrt_price_x96_to_tick her değerde güvenli olmalı ─
        #[test]
        fn stres_sqrt_price_x96_to_tick(
            sqrt_price_x96 in arb_sqrt_price_x96(),
        ) {
            let sonuc = sqrt_price_x96_to_tick(sqrt_price_x96);
            // Geçerli Uniswap V3 tick aralığı
            prop_assert!(sonuc >= -887272 && sonuc <= 887272,
                "sqrt_price_x96_to_tick aralık dışı! sqrt_price={}, tick={}",
                sqrt_price_x96, sonuc);
        }

        // ─── TEST 6: max_safe_swap_amount asla NaN/Inf/panic üretmemeli ─
        #[test]
        fn stres_max_safe_swap_amount(
            sqrt_price_x96 in arb_sqrt_price_x96(),
            liquidity in arb_liquidity(),
            token0_is_weth in proptest::bool::ANY,
        ) {
            // Derive a reasonable tick from sqrtPrice for test
            let tick = sqrt_price_x96_to_tick(sqrt_price_x96);
            let sonuc = max_safe_swap_amount(
                sqrt_price_x96,
                liquidity,
                token0_is_weth,
                tick,
                10,
            );
            prop_assert!(!sonuc.is_nan(),
                "max_safe_swap_amount NaN! sqrtP={}, liq={}", sqrt_price_x96, liquidity);
            prop_assert!(!sonuc.is_infinite(),
                "max_safe_swap_amount Infinity! sqrtP={}, liq={}", sqrt_price_x96, liquidity);
            prop_assert!(sonuc >= 0.0,
                "max_safe_swap_amount negatif! sonuc={}", sonuc);
        }

        // ─── TEST 7: Bitmap'li multi-tick swap çökmemeli ──────────────
        #[test]
        fn stres_multitick_with_bitmap(
            tick in -887272..=887272i32,
            liquidity in arb_liquidity(),
            amount_in in (0.001_f64..100.0_f64),
            token0_is_weth in proptest::bool::ANY,
            tick_spacing in prop::sample::select(vec![1i32, 10, 60, 200]),
        ) {
            // Gerçekçi bir sqrtPriceX96 üret (tick'ten)
            let sqrt_price_x96 = tick_to_sqrt_price_x96(tick);
            if sqrt_price_x96 <= 0.0 || sqrt_price_x96.is_infinite() || sqrt_price_x96.is_nan() {
                return Ok(()); // Geçersiz tick, atla
            }

            // Rastgele tick etrafında küçük bir bitmap oluştur
            let mut ticks_map = HashMap::new();
            let ts = tick_spacing.max(1);
            for i in -3..=3i32 {
                let t = ((tick / ts) + i) * ts;
                ticks_map.insert(t, TickInfo {
                    liquidity_gross: 1_000_000_000_000_000_000u128,
                    liquidity_net: if i < 0 { 500_000_000_000_000_000i128 }
                                   else if i > 0 { -500_000_000_000_000_000i128 }
                                   else { 0i128 },
                    initialized: true,
                });
            }
            let bitmap = TickBitmapData {
                words: HashMap::new(),
                ticks: ticks_map,
                snapshot_block: 0,
                sync_duration_us: 0,
                scan_range: 500,
            };

            let sonuc = swap_weth_to_usdc_multitick(
                sqrt_price_x96,
                liquidity,
                tick,
                amount_in,
                0.0005,
                token0_is_weth,
                tick_spacing,
                Some(&bitmap),
            );
            prop_assert!(!sonuc.total_output.is_nan(),
                "bitmap swap NaN! tick={}, liq={}, amt={}", tick, liquidity, amount_in);
            prop_assert!(!sonuc.total_output.is_infinite(),
                "bitmap swap Infinity! tick={}, liq={}, amt={}", tick, liquidity, amount_in);
            prop_assert!(sonuc.total_output >= 0.0,
                "bitmap swap negatif! output={}", sonuc.total_output);
        }

        // ─── TEST 8: Sıfır/ekstrem likidite ile dampening swap güvenli ─
        #[test]
        fn stres_dampening_sifir_likidite(
            tick in -887272..=887272i32,
            amount_in in (0.0_f64..1e6_f64),
            token0_is_weth in proptest::bool::ANY,
        ) {
            // Sıfır likidite — kesinlikle çökmemeli, 0.0 dönmeli
            let sonuc = swap_weth_to_usdc_multitick(
                1e24_f64, // Makul bir sqrtPriceX96
                0.0,      // SIFIR likidite
                tick,
                amount_in,
                0.0005,
                token0_is_weth,
                10,
                None,
            );
            prop_assert!(!sonuc.total_output.is_nan(),
                "Sıfır liq NaN! tick={}, amt={}", tick, amount_in);
            prop_assert!(sonuc.total_output == 0.0,
                "Sıfır likiditede çıktı 0 olmalı! output={}", sonuc.total_output);

            let sonuc2 = swap_usdc_to_weth_multitick(
                1e24_f64,
                0.0,
                tick,
                amount_in,
                0.0005,
                token0_is_weth,
                10,
                None,
            );
            prop_assert!(!sonuc2.total_output.is_nan(),
                "Sıfır liq (usdc→weth) NaN! tick={}", tick);
            prop_assert!(sonuc2.total_output == 0.0,
                "Sıfır likiditede çıktı 0 olmalı! output={}", sonuc2.total_output);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  BÖLÜM: U256 EXACT-MATH — Wei Seviyesinde Hassas Swap Matematiği
// ═══════════════════════════════════════════════════════════════════════════════
//
//  Neden? EVM 256-bit tam sayılarla çalışır. f64'ün 52-bit mantissa'sı
//  18-haneli decimal hesaplamalarda yuvarlama hataları yaratır.
//  Bu modül, Uniswap V3'ün Solidity matematik kütüphanesini (TickMath,
//  SqrtPriceMath, SwapMath) birebir Rust U256'ya port eder.
//
//  Kullanım: Botun off-chain hesapladığı swap çıktısının, on-chain
//  gerçekleşecek sonuçla *wei* bazında eşleşmesi.
//
//  Kaynaklar:
//    - UniV3 TickMath.sol: getSqrtRatioAtTick, getTickAtSqrtRatio
//    - UniV3 SqrtPriceMath.sol: getNextSqrtPriceFromInput, getAmount0/1Delta
//    - UniV3 SwapMath.sol: computeSwapStep
// ═══════════════════════════════════════════════════════════════════════════════

#[allow(dead_code)]
pub mod exact {
    use alloy::primitives::U256;
    use crate::types::TickBitmapData;

    // ── Sabitler ─────────────────────────────────────────────────────────────

    /// Q96 = 2^96 (sqrtPriceX96 çözümleme sabiti)
    const Q96: U256 = U256::from_limbs([0, 0x1_0000_0000, 0, 0]); // 2^96

    /// MAX_SQRT_RATIO (UniV3 TickMath sınırı — 1461446703485210103287273052203988822378723970342)
    const MAX_SQRT_RATIO: U256 = U256::from_limbs([
        0x5D951D5263988D26, 0xEFD1FC6A50648849, 0x00000000FFFD8963, 0
    ]);

    /// MIN_SQRT_RATIO (UniV3 TickMath sınırı)
    const MIN_SQRT_RATIO: U256 = U256::from_limbs([4295128739, 0, 0, 0]);

    // ── FullMath — U256 Tam Çarpma / Bölme ──────────────────────────────────

    /// a * b / denominator (taşma güvenli, floor rounding)
    /// Uniswap V3 FullMath.mulDiv port'u.
    pub fn mul_div(a: U256, b: U256, denominator: U256) -> U256 {
        if denominator.is_zero() {
            return U256::ZERO;
        }
        if a.is_zero() || b.is_zero() {
            return U256::ZERO;
        }
        // Doğrudan çarpma dene
        if let Some(product) = a.checked_mul(b) {
            return product / denominator;
        }
        // Taşma: ayrıştırma ile hesapla
        // a * b / c = (a/c)*b + (a%c)*b/c
        let (big, small) = if a >= b { (a, b) } else { (b, a) };
        let q = big / denominator;
        let r = big % denominator;
        // q * small — genelde taşmaz (big/denom * small)
        let term1 = q.saturating_mul(small);
        // r * small / denom — r < denom, daha güvenli
        let term2 = if let Some(rs) = r.checked_mul(small) {
            rs / denominator
        } else {
            // İç içe ayrıştırma: r*small = r*(small/denom)*denom + r*(small%denom)
            let q2 = small / denominator;
            let r2 = small % denominator;
            let inner = r.saturating_mul(q2);
            let rest = if let Some(rr2) = r.checked_mul(r2) {
                rr2 / denominator
            } else {
                U256::ZERO // Aşırı nadir durum
            };
            inner + rest
        };
        term1 + term2
    }

    /// a * b / denominator (taşma güvenli, ceil rounding)
    pub fn mul_div_rounding_up(a: U256, b: U256, denominator: U256) -> U256 {
        let result = mul_div(a, b, denominator);
        // Kalan var mı kontol — varsa yukarı yuvarla
        if let Some(product) = a.checked_mul(b) {
            if product % denominator > U256::ZERO {
                return result + U256::from(1);
            }
        } else {
            // Taşma durumunda yaklaşık kontrol
            // (a*b mod c != 0 varsay — güvenli tarafta kal)
            return result + U256::from(1);
        }
        result
    }

    /// (a + b - 1) / b tarzı ceil division
    #[inline]
    pub fn div_rounding_up(a: U256, b: U256) -> U256 {
        if b.is_zero() { return U256::ZERO; }
        let d = a / b;
        if a % b > U256::ZERO { d + U256::from(1) } else { d }
    }

    // ── TickMath — Tick ↔ SqrtPriceX96 Birebir Dönüşüm ─────────────────────

    /// Tick'ten sqrtPriceX96 hesapla — UniV3 TickMath.getSqrtRatioAtTick birebir port'u.
    /// İnput: -887272 ≤ tick ≤ 887272
    /// Çıktı: uint160 sqrtPriceX96 (U256 olarak)
    pub fn get_sqrt_ratio_at_tick(tick: i32) -> U256 {
        let abs_tick = tick.unsigned_abs();
        assert!(abs_tick <= 887272, "tick aralık dışı");

        // Başlangıç ratio (Q128 formatında)
        let mut ratio: U256 = if abs_tick & 0x1 != 0 {
            U256::from_be_slice(&hex_literal::hex!("fffcb933bd6fad37aa2d162d1a594001"))
        } else {
            U256::from(1u64) << 128
        };

        // Her bit için çarpma tablosu — UniV3 TickMath magic numbers
        macro_rules! apply_tick_bit {
            ($bit:expr, $hex:expr) => {
                if abs_tick & $bit != 0 {
                    ratio = mul_div(ratio, U256::from_be_slice(&hex_literal::hex!($hex)), U256::from(1u64) << 128);
                }
            };
        }

        apply_tick_bit!(0x2,     "fff97272373d413259a46990580e213a");
        apply_tick_bit!(0x4,     "fff2e50f5f656932ef12357cf3c7fdcc");
        apply_tick_bit!(0x8,     "ffe5caca7e10e4e61c3624eaa0941cd0");
        apply_tick_bit!(0x10,    "ffcb9843d60f6159c9db58835c926644");
        apply_tick_bit!(0x20,    "ff973b41fa98c081472e6896dfb254c0");
        apply_tick_bit!(0x40,    "ff2ea16466c96a3843ec78b326b52861");
        apply_tick_bit!(0x80,    "fe5dee046a99a2a811c461f1969c3053");
        apply_tick_bit!(0x100,   "fcbe86c7900a88aedcffc83b479aa3a4");
        apply_tick_bit!(0x200,   "f987a7253ac413176f2b074cf7815e54");
        apply_tick_bit!(0x400,   "f3392b0822b70005940c7a398e4b70f3");
        apply_tick_bit!(0x800,   "e7159475a2c29b7443b29c7fa6e889d9");
        apply_tick_bit!(0x1000,  "d097f3bdfd2022b8845ad8f792aa5825");
        apply_tick_bit!(0x2000,  "a9f746462d870fdf8a65dc1f90e061e5");
        apply_tick_bit!(0x4000,  "70d869a156d2a1b890bb3df62baf32f7");
        apply_tick_bit!(0x8000,  "31be135f97d08fd981231505542fcfa6");
        apply_tick_bit!(0x10000, "09aa508b5b7a84e1c677de54f3e99bc9");
        apply_tick_bit!(0x20000, "005d6af8dedb81196699c329225ee604");
        apply_tick_bit!(0x40000, "00002216e584f5fa1ea926041bedfe98");
        apply_tick_bit!(0x80000, "048a170391f7dc42444e8fa2");

        // Pozitif tick → ters çevir
        if tick > 0 {
            ratio = U256::MAX / ratio;
        }

        // Q128 → Q96 dönüşümü + yukarı yuvarlama
        let remainder = ratio % (U256::from(1u64) << 32);
        let shifted = ratio >> 32;
        if remainder > U256::ZERO {
            shifted + U256::from(1)
        } else {
            shifted
        }
    }

    // ── SqrtPriceMath — Fiyat Geçişi Hesaplamaları ─────────────────────────

    /// token0 girdisi ile yeni sqrtPrice hesapla (zeroForOne=true, fiyat DÜŞER)
    /// Port: SqrtPriceMath.getNextSqrtPriceFromAmount0RoundingUp
    pub fn get_next_sqrt_price_from_amount0(
        sqrt_price_x96: U256,
        liquidity: u128,
        amount: U256,
        add: bool,
    ) -> U256 {
        if amount.is_zero() {
            return sqrt_price_x96;
        }
        let numerator1 = U256::from(liquidity) << 96;

        if add {
            // sqrtPriceNext = numerator1 * sqrtP / (numerator1 + amount * sqrtP)
            let product = mul_div(amount, sqrt_price_x96, U256::from(1u64));
            let denominator = numerator1 + product;
            if denominator >= numerator1 {
                return mul_div_rounding_up(numerator1, sqrt_price_x96, denominator);
            }
            // Taşma fallback
            div_rounding_up(numerator1, numerator1 / sqrt_price_x96 + amount)
        } else {
            // sqrtPriceNext = numerator1 * sqrtP / (numerator1 - amount * sqrtP)
            let product = mul_div(amount, sqrt_price_x96, U256::from(1u64));
            if numerator1 <= product {
                return U256::ZERO; // Yetersiz likidite
            }
            let denominator = numerator1 - product;
            mul_div_rounding_up(numerator1, sqrt_price_x96, denominator)
        }
    }

    /// token1 girdisi ile yeni sqrtPrice hesapla (zeroForOne=false, fiyat ARTAR)
    /// Port: SqrtPriceMath.getNextSqrtPriceFromAmount1RoundingDown
    pub fn get_next_sqrt_price_from_amount1(
        sqrt_price_x96: U256,
        liquidity: u128,
        amount: U256,
        add: bool,
    ) -> U256 {
        if add {
            let quotient = mul_div(amount, Q96, U256::from(liquidity));
            sqrt_price_x96 + quotient
        } else {
            let quotient = mul_div_rounding_up(amount, Q96, U256::from(liquidity));
            if sqrt_price_x96 <= quotient {
                return U256::ZERO;
            }
            sqrt_price_x96 - quotient
        }
    }

    /// Girdi miktarından yeni sqrtPrice hesapla (yön'e göre dispatch)
    pub fn get_next_sqrt_price_from_input(
        sqrt_price_x96: U256,
        liquidity: u128,
        amount_in: U256,
        zero_for_one: bool,
    ) -> U256 {
        if zero_for_one {
            get_next_sqrt_price_from_amount0(sqrt_price_x96, liquidity, amount_in, true)
        } else {
            get_next_sqrt_price_from_amount1(sqrt_price_x96, liquidity, amount_in, true)
        }
    }

    /// İki sqrtPrice arasındaki token0 farkı (Δx)
    /// Port: SqrtPriceMath.getAmount0Delta (unsigned)
    pub fn get_amount0_delta(
        sqrt_ratio_a: U256,
        sqrt_ratio_b: U256,
        liquidity: u128,
        round_up: bool,
    ) -> U256 {
        let (lower, upper) = if sqrt_ratio_a < sqrt_ratio_b {
            (sqrt_ratio_a, sqrt_ratio_b)
        } else {
            (sqrt_ratio_b, sqrt_ratio_a)
        };
        if lower.is_zero() { return U256::ZERO; }

        let numerator1 = U256::from(liquidity) << 96;
        let numerator2 = upper - lower;

        if round_up {
            div_rounding_up(
                mul_div_rounding_up(numerator1, numerator2, upper),
                lower,
            )
        } else {
            mul_div(numerator1, numerator2, upper) / lower
        }
    }

    /// İki sqrtPrice arasındaki token1 farkı (Δy)
    /// Port: SqrtPriceMath.getAmount1Delta (unsigned)
    pub fn get_amount1_delta(
        sqrt_ratio_a: U256,
        sqrt_ratio_b: U256,
        liquidity: u128,
        round_up: bool,
    ) -> U256 {
        let (lower, upper) = if sqrt_ratio_a < sqrt_ratio_b {
            (sqrt_ratio_a, sqrt_ratio_b)
        } else {
            (sqrt_ratio_b, sqrt_ratio_a)
        };

        if round_up {
            mul_div_rounding_up(U256::from(liquidity), upper - lower, Q96)
        } else {
            mul_div(U256::from(liquidity), upper - lower, Q96)
        }
    }

    // ── SwapMath — Tek Adım Swap Hesaplama ──────────────────────────────────

    /// Tek bir fiyat aralığındaki swap adımı sonucu
    #[derive(Debug, Clone)]
    pub struct ExactSwapStep {
        /// Swap sonrası sqrtPriceX96
        pub sqrt_ratio_next: U256,
        /// Tüketilen girdi miktarı
        pub amount_in: U256,
        /// Üretilen çıktı miktarı
        pub amount_out: U256,
        /// Alınan fee miktarı
        pub fee_amount: U256,
    }

    /// Tek fiyat aralığında swap adımı hesapla
    /// Port: SwapMath.computeSwapStep
    pub fn compute_swap_step(
        sqrt_ratio_current: U256,
        sqrt_ratio_target: U256,
        liquidity: u128,
        amount_remaining: U256,
        fee_pips: u32, // 1e6 bazında (ör: 500 = %0.05)
    ) -> ExactSwapStep {
        let zero_for_one = sqrt_ratio_current >= sqrt_ratio_target;
        let one_minus_fee = U256::from(1_000_000u64 - fee_pips as u64);

        // Fee düşülmüş efektif girdi
        let amount_remaining_less_fee = mul_div(
            amount_remaining, one_minus_fee, U256::from(1_000_000u64),
        );

        // Bu aralıktaki maksimum girdi
        let amount_in_max = if zero_for_one {
            get_amount0_delta(sqrt_ratio_target, sqrt_ratio_current, liquidity, true)
        } else {
            get_amount1_delta(sqrt_ratio_current, sqrt_ratio_target, liquidity, true)
        };

        // Hedef fiyata ulaşabilir miyiz?
        let sqrt_ratio_next = if amount_remaining_less_fee >= amount_in_max {
            sqrt_ratio_target
        } else {
            get_next_sqrt_price_from_input(
                sqrt_ratio_current, liquidity, amount_remaining_less_fee, zero_for_one,
            )
        };

        let max_reached = sqrt_ratio_next == sqrt_ratio_target;

        // Gerçek girdi ve çıktı miktarlarını hesapla
        let amount_in = if max_reached {
            amount_in_max
        } else if zero_for_one {
            get_amount0_delta(sqrt_ratio_next, sqrt_ratio_current, liquidity, true)
        } else {
            get_amount1_delta(sqrt_ratio_current, sqrt_ratio_next, liquidity, true)
        };

        let amount_out = if zero_for_one {
            get_amount1_delta(sqrt_ratio_next, sqrt_ratio_current, liquidity, false)
        } else {
            get_amount0_delta(sqrt_ratio_current, sqrt_ratio_next, liquidity, false)
        };

        // Fee hesapla
        let fee_amount = if !max_reached {
            amount_remaining - amount_in
        } else {
            mul_div_rounding_up(amount_in, U256::from(fee_pips), one_minus_fee)
        };

        ExactSwapStep {
            sqrt_ratio_next,
            amount_in,
            amount_out,
            fee_amount,
        }
    }

    // ── Tam Multi-Tick Swap Simülasyonu (Exact) ─────────────────────────────

    /// Exact multi-tick swap sonucu (U256 hassasiyetinde)
    #[derive(Debug, Clone)]
    pub struct ExactSwapResult {
        /// Toplam çıktı miktarı (raw wei)
        pub amount_out: U256,
        /// Toplam tüketilen girdi (raw wei, fee dahil)
        pub amount_in_consumed: U256,
        /// Son sqrtPriceX96
        pub final_sqrt_price_x96: U256,
        /// Son likidite
        pub final_liquidity: u128,
        /// Geçilen tick sayısı
        pub tick_crossings: u32,
    }

    /// Exact V3/CL swap simülasyonu — U256 hassasiyetinde, wei-bazında eşleşme.
    ///
    /// Bu fonksiyon Uniswap V3'ün on-chain `swap()` fonksiyonunun
    /// matematiğini birebir taklit eder:
    ///   1. Mevcut fiyat aralığında ne kadar swap yapılabilir? (computeSwapStep)
    ///   2. Girdi tükenmezse sonraki tick'e ilerle
    ///   3. O tick'te liquidityNet ile aktif likiditeyi güncelle
    ///   4. Tekrarla
    ///
    /// # Parametreler
    /// - `sqrt_price_x96`: Mevcut sqrtPriceX96 (U256)
    /// - `liquidity`: Mevcut aktif likidite (u128)
    /// - `amount_in`: Girdi miktarı (wei, fee dahil ham miktar)
    /// - `zero_for_one`: Swap yönü (true=token0→token1, false=token1→token0)
    /// - `fee_pips`: Fee (1e6 bazında, ör: 500 = %0.05)
    /// - `tick_spacing`: Tick aralığı
    /// - `bitmap`: TickBitmap verisi (başlatılmış tick'ler + liquidityNet)
    pub fn compute_exact_swap(
        sqrt_price_x96: U256,
        liquidity: u128,
        current_tick: i32,
        amount_in: U256,
        zero_for_one: bool,
        fee_pips: u32,
        _tick_spacing: i32,
        bitmap: Option<&TickBitmapData>,
    ) -> ExactSwapResult {
        if amount_in.is_zero() || liquidity == 0 || sqrt_price_x96.is_zero() {
            return ExactSwapResult {
                amount_out: U256::ZERO,
                amount_in_consumed: U256::ZERO,
                final_sqrt_price_x96: sqrt_price_x96,
                final_liquidity: liquidity,
                tick_crossings: 0,
            };
        }

        let mut state_sqrt_price = sqrt_price_x96;
        let mut state_liquidity = liquidity;
        let mut state_tick = current_tick;
        let mut amount_remaining = amount_in;
        let mut total_amount_out = U256::ZERO;
        let mut total_amount_in = U256::ZERO;
        let mut crossings: u32 = 0;
        let max_crossings: u32 = 50;

        // Sıralı tick'leri al
        let ordered_ticks = if let Some(bm) = bitmap {
            let mut ticks: Vec<(i32, i128)> = bm.ticks.iter()
                .filter(|(_, info)| info.initialized)
                .map(|(&t, info)| (t, info.liquidity_net))
                .collect();

            if zero_for_one {
                ticks.retain(|(t, _)| *t <= state_tick);
                ticks.sort_by(|a, b| b.0.cmp(&a.0));
            } else {
                ticks.retain(|(t, _)| *t > state_tick);
                ticks.sort_by_key(|(t, _)| *t);
            }
            ticks
        } else {
            Vec::new()
        };

        // Ana swap döngüsü — tick'ler boyunca ilerle
        for &(next_tick, liquidity_net) in &ordered_ticks {
            if amount_remaining.is_zero() || crossings >= max_crossings {
                break;
            }

            // Hedef tick sınırının sqrtPrice'ını hesapla
            let sqrt_price_target = get_sqrt_ratio_at_tick(next_tick);

            // Bu aralıkta swap yap
            let step = compute_swap_step(
                state_sqrt_price,
                sqrt_price_target,
                state_liquidity,
                amount_remaining,
                fee_pips,
            );

            total_amount_out += step.amount_out;
            let consumed = step.amount_in + step.fee_amount;
            total_amount_in += consumed;
            if amount_remaining >= consumed {
                amount_remaining -= consumed;
            } else {
                amount_remaining = U256::ZERO;
            }
            state_sqrt_price = step.sqrt_ratio_next;

            // Tick sınırına ulaştık mı?
            if step.sqrt_ratio_next == sqrt_price_target && !amount_remaining.is_zero() {
                // Likiditeyi güncelle
                if zero_for_one {
                    if state_liquidity as i128 >= liquidity_net {
                        state_liquidity = (state_liquidity as i128 - liquidity_net) as u128;
                    } else {
                        state_liquidity = 0;
                    }
                } else {
                    let new_liq = state_liquidity as i128 + liquidity_net;
                    state_liquidity = if new_liq > 0 { new_liq as u128 } else { 0 };
                }
                state_tick = if zero_for_one { next_tick - 1 } else { next_tick };
                let _ = state_tick; // tick crossing sırasında güncellenir
                crossings += 1;
            }
        }

        // Kalan girdi varsa mevcut likiditede son bir adım daha
        if !amount_remaining.is_zero() && state_liquidity > 0 {
            let final_target = if zero_for_one { MIN_SQRT_RATIO + U256::from(1) } else { MAX_SQRT_RATIO - U256::from(1) };
            let step = compute_swap_step(
                state_sqrt_price,
                final_target,
                state_liquidity,
                amount_remaining,
                fee_pips,
            );
            total_amount_out += step.amount_out;
            total_amount_in += step.amount_in + step.fee_amount;
            state_sqrt_price = step.sqrt_ratio_next;
        }

        ExactSwapResult {
            amount_out: total_amount_out,
            amount_in_consumed: total_amount_in,
            final_sqrt_price_x96: state_sqrt_price,
            final_liquidity: state_liquidity,
            tick_crossings: crossings,
        }
    }

    /// İki havuz arasında exact arbitraj kârı hesapla (U256, wei bazında)
    ///
    /// Akış:
    ///   1. Pahalı havuzda WETH sat → USDC al (exact V3 swap)
    ///   2. Ucuz havuzda USDC ile WETH geri al (exact V3 swap)
    ///   3. Flash loan geri ödeme çıkar
    ///   4. Kalan = net kâr (owed token cinsinden, wei)
    ///
    /// # Dönüş
    /// (net_profit_wei, amount_received_from_second_swap)
    /// Kâr yoksa (0, 0) döner.
    pub fn compute_exact_arbitrage_profit(
        // Pahalı havuz (satış hedefi)
        sell_sqrt_price: U256,
        sell_liquidity: u128,
        sell_tick: i32,
        sell_fee_pips: u32,
        sell_tick_spacing: i32,
        sell_bitmap: Option<&TickBitmapData>,
        // Ucuz havuz (alım hedefi)
        buy_sqrt_price: U256,
        buy_liquidity: u128,
        buy_tick: i32,
        buy_fee_pips: u32,
        buy_tick_spacing: i32,
        buy_bitmap: Option<&TickBitmapData>,
        // İşlem parametreleri
        amount_in_wei: U256,
        token0_is_weth: bool,
    ) -> (U256, U256) {
        // Adım 1: Pahalı havuzda WETH → USDC
        let sell_zero_for_one = token0_is_weth; // token0=WETH → zeroForOne=true
        let sell_result = compute_exact_swap(
            sell_sqrt_price, sell_liquidity, sell_tick,
            amount_in_wei, sell_zero_for_one,
            sell_fee_pips, sell_tick_spacing, sell_bitmap,
        );

        if sell_result.amount_out.is_zero() {
            return (U256::ZERO, U256::ZERO);
        }

        // Adım 2: Ucuz havuzda USDC → WETH
        let buy_zero_for_one = !token0_is_weth; // USDC girdi → zeroForOne = !token0_is_weth
        let buy_result = compute_exact_swap(
            buy_sqrt_price, buy_liquidity, buy_tick,
            sell_result.amount_out, buy_zero_for_one,
            buy_fee_pips, buy_tick_spacing, buy_bitmap,
        );

        if buy_result.amount_out.is_zero() {
            return (U256::ZERO, U256::ZERO);
        }

        // Adım 3: Net kâr = WETH alınan - WETH borçlu (flash loan)
        if buy_result.amount_out > amount_in_wei {
            (buy_result.amount_out - amount_in_wei, buy_result.amount_out)
        } else {
            (U256::ZERO, buy_result.amount_out)
        }
    }

    // ── Yön-Bazlı Exact Kâr Hesaplama (Flash Swap Akışı Birebir Model) ─────

    /// Flash swap akışını birebir modelleyerek kârı hesapla.
    ///
    /// Kontrat akışı:
    ///   1. UniV3(PoolA): amount_wei input → received_amount output
    ///   2. Slipstream(PoolB): received_amount input → owed_output
    ///   3. profit = owed_output - amount_wei (owedToken cinsinden, wei)
    ///
    /// Bu fonksiyon hem WETH-ödeme hem USDC-ödeme senaryolarını doğru hesaplar.
    /// minProfit calldata değeri bu fonksiyonun çıktısından türetilir.
    ///
    /// # Dönüş
    /// Kâr U256 (owedToken cinsinden, wei). Kâr yoksa U256::ZERO.
    pub fn compute_exact_directional_profit(
        // Pool A (UniV3 — flash swap kaynağı)
        pool_a_sqrt_price: U256,
        pool_a_liquidity: u128,
        pool_a_tick: i32,
        pool_a_fee_pips: u32,
        pool_a_tick_spacing: i32,
        pool_a_bitmap: Option<&TickBitmapData>,
        // Pool B (Slipstream — satış hedefi)
        pool_b_sqrt_price: U256,
        pool_b_liquidity: u128,
        pool_b_tick: i32,
        pool_b_fee_pips: u32,
        pool_b_tick_spacing: i32,
        pool_b_bitmap: Option<&TickBitmapData>,
        // Swap parametreleri
        amount_wei: U256,
        uni_zero_for_one: bool,
        aero_zero_for_one: bool,
    ) -> U256 {
        if amount_wei.is_zero() {
            return U256::ZERO;
        }

        // Adım 1: UniV3 flash swap
        // amount_wei input → received tokens output
        let univ3_result = compute_exact_swap(
            pool_a_sqrt_price,
            pool_a_liquidity,
            pool_a_tick,
            amount_wei,
            uni_zero_for_one,
            pool_a_fee_pips,
            pool_a_tick_spacing,
            pool_a_bitmap,
        );

        if univ3_result.amount_out.is_zero() {
            return U256::ZERO;
        }

        // Adım 2: Slipstream swap
        // UniV3'ten alınan tokenlar → owedToken geri alınır
        let slipstream_result = compute_exact_swap(
            pool_b_sqrt_price,
            pool_b_liquidity,
            pool_b_tick,
            univ3_result.amount_out,
            aero_zero_for_one,
            pool_b_fee_pips,
            pool_b_tick_spacing,
            pool_b_bitmap,
        );

        // Adım 3: Kâr = Slipstream çıktısı - UniV3'e borç
        // Kontrat akışı: balAfter(owedToken) - balBefore(owedToken)
        // owed_output (Slipstream'den) - amount_wei (UniV3'e ödeme)
        if slipstream_result.amount_out > amount_wei {
            slipstream_result.amount_out - amount_wei
        } else {
            U256::ZERO
        }
    }

    // ── Dönüşüm Yardımcıları ────────────────────────────────────────────────

    /// U256'yı f64'e güvenli dönüştür.
    /// v22.0: String conversion → doğrudan bit manipülasyonu.
    /// Eski: val.to_string().parse::<f64>() — her dönüşümde heap allocation.
    /// Yeni: U256 limb'lerinden doğrudan f64 hesaplama — zero-alloc.
    /// Not: 2^53 üstü değerlerde düşük bitler kaybolur ama WETH/USD
    /// aralığındaki wei değerleri için sorun oluşturmaz.
    pub fn u256_to_f64(val: U256) -> f64 {
        if val.is_zero() {
            return 0.0;
        }
        // U256 → [u64; 4] limbs (little-endian)
        let limbs = val.as_limbs();
        // En yüksek anlamlı limb'i bul
        if limbs[3] != 0 {
            // 192-255 bit aralığında
            limbs[3] as f64 * (2.0f64).powi(192)
                + limbs[2] as f64 * (2.0f64).powi(128)
                + limbs[1] as f64 * (2.0f64).powi(64)
                + limbs[0] as f64
        } else if limbs[2] != 0 {
            // 128-191 bit aralığında
            limbs[2] as f64 * (2.0f64).powi(128)
                + limbs[1] as f64 * (2.0f64).powi(64)
                + limbs[0] as f64
        } else if limbs[1] != 0 {
            // 64-127 bit aralığında
            limbs[1] as f64 * (2.0f64).powi(64)
                + limbs[0] as f64
        } else {
            // 0-63 bit aralığında — tam hassasiyet (f64 mantissa 53 bit)
            limbs[0] as f64
        }
    }

    /// Fee fraction (ör: 0.0005) → fee pips (ör: 500, 1e6 bazında).
    /// Uniswap V3 fee_pips: 500 = %0.05, 3000 = %0.30, 10000 = %1.00
    #[inline]
    pub fn fee_fraction_to_pips(fee_fraction: f64) -> u32 {
        (fee_fraction * 1_000_000.0).round() as u32
    }

    /// U256-tabanlı güvenli maksimum swap miktarı (f64 WETH döndürür).
    /// Mevcut tick aralığındaki likidite kapasitesinin %15'ini hesaplar.
    ///
    /// Uniswap V3 SqrtPriceMath formülleri ile:
    ///   token0: capacity = L × Q96 / sqrtPriceX96
    ///   token1: capacity = L × sqrtPriceX96 / Q96
    ///
    /// v11.1: Artık %15 sezgisel hesap YOK.  Bunun yerine mevcut fiyattan
    ///        bir sonraki tick_spacing sınırına kadar absorbe edilebilecek
    ///        tam WETH miktarı hesaplanır (get_amount0_delta / get_amount1_delta).
    ///        Böylece 6 vs 18 decimal asimetrik havuzlarda milyarlık hayalet
    ///        likidite sorunu ortadan kalkar.
    pub fn max_safe_swap_amount_u256(
        sqrt_price_x96: U256,
        liquidity: u128,
        token0_is_weth: bool,
        current_tick: i32,
        tick_spacing: i32,
    ) -> f64 {
        if sqrt_price_x96.is_zero() || liquidity == 0 || tick_spacing == 0 {
            return 0.0;
        }

        // Tick'i Uniswap V3 geçerli aralığa kısıtla
        const MIN_TICK: i32 = -887272;
        const MAX_TICK: i32 = 887272;
        let clamped_tick = current_tick.clamp(MIN_TICK, MAX_TICK);

        // Mevcut tick aralığının alt ve üst sınırlarını bul
        let lower_tick = clamped_tick.div_euclid(tick_spacing) * tick_spacing;
        let upper_tick = lower_tick + tick_spacing;

        let capacity_raw = if token0_is_weth {
            // WETH = token0 → swap yönü zeroForOne → fiyat DÜŞER (alt sınıra)
            let sqrt_lower = get_sqrt_ratio_at_tick(lower_tick.clamp(MIN_TICK, MAX_TICK));
            // Fiyat tam sınırdaysa bir tick_spacing daha aşağı git
            let target_tick = if sqrt_lower >= sqrt_price_x96 {
                (lower_tick - tick_spacing).clamp(MIN_TICK, MAX_TICK)
            } else {
                lower_tick
            };
            let sqrt_target = get_sqrt_ratio_at_tick(target_tick);
            if sqrt_target >= sqrt_price_x96 {
                return 0.0;
            }
            get_amount0_delta(sqrt_target, sqrt_price_x96, liquidity, false)
        } else {
            // WETH = token1 → swap yönü oneForZero → fiyat YUKARI (üst sınıra)
            let sqrt_upper = get_sqrt_ratio_at_tick(upper_tick.clamp(MIN_TICK, MAX_TICK));
            // Fiyat tam sınırdaysa bir tick_spacing daha yukarı git
            let target_tick = if sqrt_upper <= sqrt_price_x96 {
                (upper_tick + tick_spacing).clamp(MIN_TICK, MAX_TICK)
            } else {
                upper_tick.clamp(MIN_TICK, MAX_TICK)
            };
            let sqrt_target = get_sqrt_ratio_at_tick(target_tick);
            if sqrt_target <= sqrt_price_x96 {
                return 0.0;
            }
            get_amount1_delta(sqrt_price_x96, sqrt_target, liquidity, false)
        };

        u256_to_f64(capacity_raw) / 1e18
    }

    /// Hard Liquidity Cap — TickBitmap'ten gerçek mevcut likiditeyi hesapla.
    ///
    /// Bu fonksiyon, mevcut tick'ten itibaren swap yönündeki tüm başlatılmış
    /// tick'lerdeki toplam absorbe edilebilir WETH miktarını hesaplar.
    ///
    /// Algoritma:
    ///   1. Swap yönüne göre (zeroForOne veya oneForZero) ilgili tick'leri sırala
    ///   2. Her tick aralığındaki mevcut likidite ile o aralıkta absorbe
    ///      edilebilecek maksimum WETH miktarını SqrtPriceMath ile hesapla
    ///   3. Tick sınırında liquidityNet ile aktif likiditeyi güncelle
    ///   4. Tüm aralıklardaki kapasiteleri topla
    ///
    /// Bu sayede NR, havuzda gerçekten mevcut olmayan likiditeyi
    /// kullanmaya çalışmaz. Örn: 5.5 WETH likidite varsa max 5.5 WETH önerilir.
    ///
    /// # Dönüş
    /// Toplam absorbe edilebilir WETH miktarı (f64, human-readable).
    /// Bitmap yoksa veya boşsa, `max_safe_swap_amount_u256` fallback kullanılır.
    pub fn hard_liquidity_cap_weth(
        sqrt_price_x96: U256,
        liquidity: u128,
        current_tick: i32,
        token0_is_weth: bool,
        bitmap: Option<&TickBitmapData>,
        tick_spacing: i32,
    ) -> f64 {
        // Bitmap yoksa single-tick fallback
        let bitmap = match bitmap {
            Some(bm) if !bm.ticks.is_empty() => bm,
            _ => return max_safe_swap_amount_u256(sqrt_price_x96, liquidity, token0_is_weth, current_tick, tick_spacing),
        };

        if sqrt_price_x96.is_zero() || liquidity == 0 {
            return 0.0;
        }

        // WETH satışı: zeroForOne (token0_is_weth=true → sola git) veya
        //               oneForZero (token0_is_weth=false → sağa git)
        // Bot WETH giriyor → swap yönü WETH→USDC
        let zero_for_one = token0_is_weth;

        // Tick'leri swap yönüne göre sırala
        let ordered_ticks: Vec<(i32, i128)> = {
            let mut ticks: Vec<(i32, i128)> = bitmap.ticks.iter()
                .filter(|(_, info)| info.initialized)
                .map(|(&t, info)| (t, info.liquidity_net))
                .collect();

            if zero_for_one {
                ticks.retain(|(t, _)| *t <= current_tick);
                ticks.sort_by(|a, b| b.0.cmp(&a.0)); // büyükten küçüğe
            } else {
                ticks.retain(|(t, _)| *t > current_tick);
                ticks.sort_by_key(|(t, _)| *t); // küçükten büyüğe
            }
            ticks
        };

        let mut state_sqrt_price = sqrt_price_x96;
        let mut state_liquidity = liquidity;
        let mut total_weth_capacity = U256::ZERO;
        let max_crossings = 50u32;

        for (i, &(next_tick, liquidity_net)) in ordered_ticks.iter().enumerate() {
            if i as u32 >= max_crossings || state_liquidity == 0 {
                break;
            }

            let sqrt_price_target = get_sqrt_ratio_at_tick(next_tick);

            // Bu aralıkta absorbe edilebilecek WETH miktarı
            let weth_in_range = if zero_for_one {
                // token0(WETH) girdi: amount0 = L × Q96 × (1/target - 1/current)
                if sqrt_price_target < state_sqrt_price {
                    get_amount0_delta(sqrt_price_target, state_sqrt_price, state_liquidity, false)
                } else {
                    U256::ZERO
                }
            } else {
                // token1(WETH) girdi: amount1 = L × (target - current) / Q96
                if sqrt_price_target > state_sqrt_price {
                    get_amount1_delta(state_sqrt_price, sqrt_price_target, state_liquidity, false)
                } else {
                    U256::ZERO
                }
            };

            total_weth_capacity += weth_in_range;
            state_sqrt_price = sqrt_price_target;

            // Tick sınırında likiditeyi güncelle
            if zero_for_one {
                if state_liquidity as i128 >= liquidity_net {
                    state_liquidity = (state_liquidity as i128 - liquidity_net) as u128;
                } else {
                    state_liquidity = 0;
                }
            } else {
                let new_liq = state_liquidity as i128 + liquidity_net;
                state_liquidity = if new_liq > 0 { new_liq as u128 } else { 0 };
            }
        }

        // Son tick'ten sonra kalan likiditede de bir miktar daha absorbe edilebilir
        // ama muhafazakâr olalım — sadece başlatılmış tick'lere kadar hesapla

        let cap_weth = u256_to_f64(total_weth_capacity) / 1e18;

        // Minimum: single-tick fallback ile karşılaştır, büyük olanı al
        // (bitmap'te çok az tick varsa fallback daha iyi olabilir)
        let single_tick_cap = max_safe_swap_amount_u256(sqrt_price_x96, liquidity, token0_is_weth, current_tick, tick_spacing);

        // v16.0: %99.9 güvenlik marjı — slippage payı bırak.
        // Havuz kapasitesinin tam sınırında işlem yapmak tick-çapraz
        // hatalarına ve REVM revert'lerine yol açar.
        let raw_cap = cap_weth.max(single_tick_cap);
        raw_cap * 0.999
    }

    // ── Test ─────────────────────────────────────────────────────────────────

    #[cfg(test)]
    mod exact_tests {
        use super::*;
        use alloy::primitives::U256;

        #[test]
        fn test_get_sqrt_ratio_at_tick_zero() {
            let ratio = get_sqrt_ratio_at_tick(0);
            // tick=0 → sqrtPrice = 2^96 = Q96
            assert_eq!(ratio, Q96, "tick=0 → sqrtPriceX96 = Q96");
        }

        #[test]
        fn test_get_sqrt_ratio_at_tick_boundaries() {
            let min_ratio = get_sqrt_ratio_at_tick(-887272);
            let max_ratio = get_sqrt_ratio_at_tick(887272);
            assert!(min_ratio >= MIN_SQRT_RATIO, "min tick → min sqrt ratio");
            assert!(max_ratio <= MAX_SQRT_RATIO, "max tick → max sqrt ratio");
            assert!(min_ratio < max_ratio, "min < max");
        }

        #[test]
        fn test_get_sqrt_ratio_negative_tick() {
            let ratio_neg = get_sqrt_ratio_at_tick(-1);
            let ratio_pos = get_sqrt_ratio_at_tick(1);
            let ratio_zero = get_sqrt_ratio_at_tick(0);
            assert!(ratio_neg < ratio_zero, "negatif tick → düşük fiyat");
            assert!(ratio_pos > ratio_zero, "pozitif tick → yüksek fiyat");
        }

        #[test]
        fn test_mul_div_basic() {
            let a = U256::from(1000u64);
            let b = U256::from(2000u64);
            let c = U256::from(500u64);
            assert_eq!(mul_div(a, b, c), U256::from(4000u64));
        }

        #[test]
        fn test_mul_div_large_numbers() {
            let a = U256::from(1u64) << 200;
            let b = U256::from(3u64);
            let c = U256::from(2u64);
            let expected = (U256::from(1u64) << 200) + (U256::from(1u64) << 199);
            assert_eq!(mul_div(a, b, c), expected);
        }

        #[test]
        fn test_compute_swap_step_basic() {
            let sqrt_price = get_sqrt_ratio_at_tick(0);
            let target = get_sqrt_ratio_at_tick(-10);
            let liquidity: u128 = 1_000_000_000_000_000_000; // 1e18
            let amount = U256::from(1_000_000u64); // 1 USDC worth

            let step = compute_swap_step(sqrt_price, target, liquidity, amount, 500);
            assert!(step.amount_out > U256::ZERO, "Çıktı sıfır olmamalı");
            assert!(step.amount_in > U256::ZERO, "Girdi sıfır olmamalı");
        }

        #[test]
        fn test_exact_swap_no_bitmap() {
            let sqrt_price = get_sqrt_ratio_at_tick(-200000);
            let liquidity: u128 = 50_000_000_000_000_000_000; // 5e19
            let amount = U256::from(1_000_000_000_000_000_000u128); // 1 WETH

            let result = compute_exact_swap(
                sqrt_price, liquidity, -200000,
                amount, true, 500, 10, None,
            );

            assert!(result.amount_out > U256::ZERO, "Swap çıktısı > 0 olmalı");
            println!("Exact swap: 1 WETH → {} raw USDC çıktı", result.amount_out);
        }

        #[test]
        fn test_exact_swap_with_bitmap() {
            use std::collections::HashMap;
            use crate::types::TickInfo;

            let tick = -200000i32;
            let sqrt_price = get_sqrt_ratio_at_tick(tick);
            let liquidity: u128 = 50_000_000_000_000_000_000;

            let mut ticks = HashMap::new();
            let ts = 10;
            for i in -5..=5i32 {
                let t = ((tick / ts) + i) * ts;
                ticks.insert(t, TickInfo {
                    liquidity_gross: 5_000_000_000_000_000_000u128,
                    liquidity_net: if i < 0 { 5_000_000_000_000_000_000i128 }
                                   else if i > 0 { -5_000_000_000_000_000_000i128 }
                                   else { 0 },
                    initialized: true,
                });
            }
            let bitmap = TickBitmapData {
                words: HashMap::new(),
                ticks,
                snapshot_block: 0,
                sync_duration_us: 0,
                scan_range: 500,
            };

            let amount = U256::from(5_000_000_000_000_000_000u128); // 5 WETH
            let result = compute_exact_swap(
                sqrt_price, liquidity, tick,
                amount, true, 500, 10, Some(&bitmap),
            );

            assert!(result.amount_out > U256::ZERO, "Bitmap swap çıktısı > 0");
            assert!(result.tick_crossings > 0, "Tick geçişi olmalı");
            println!(
                "Exact bitmap swap: 5 WETH → {} raw çıktı, {} tick geçişi",
                result.amount_out, result.tick_crossings
            );
        }
    }
}
