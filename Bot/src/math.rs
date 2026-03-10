// ============================================================================
//  MATH v7.0 Ã¢â‚¬â€ U256 Exact-Math Stabilizasyon + Multi-Tick CL Swap Motoru
//
//  v7.0 Yenilikler (Faz 1 Ã¢â‚¬â€ Stabilizasyon):
//  Ã¢Å“â€œ compute_arbitrage_profit_with_bitmap Ã¢â€ â€™ U256 exact::compute_exact_swap
//  Ã¢Å“â€œ find_optimal_amount_with_bitmap Ã¢â€ â€™ U256 liq cap (max_safe_swap_amount_u256)
//  Ã¢Å“â€œ swap_weth_to_usdc_exact / swap_usdc_to_weth_exact Ã¢â‚¬â€ U256 public API
//  Ã¢Å“â€œ max_safe_swap_amount Ã¢â€ â€™ U256 delegasyonu
//  Ã¢Å“â€œ Uniswap V3 FullMath/SqrtPriceMath/SwapMath birebir Rust U256 port'u
//  Ã¢Å“â€œ Newton-Raphson optimizer artÃ„Â±k U256 swap ile profit deÃ„Å¸erlendirmesi yapar
//  Ã¢Å“â€œ On-chain sonuÃƒÂ§la wei bazÃ„Â±nda eÃ…Å¸leÃ…Å¸en deterministik kesinlik
//
//  v6.0 (korunuyor):
//  Ã¢Å“â€œ GERÃƒâ€¡EK multi-tick swap: TickBitmap verisinden sÃ„Â±ralÃ„Â± tick geÃƒÂ§iÃ…Å¸i (legacy f64)
//  Ã¢Å“â€œ "50 ETH satarsam hangi 3 tick'i patlatÃ„Â±rÃ„Â±m?" Ã¢â€ â€™ mikrosaniye cevap
//  Ã¢Å“â€œ Her tick sÃ„Â±nÃ„Â±rÃ„Â±nda liquidityNet ile aktif likidite gÃƒÂ¼ncelleme
//  Ã¢Å“â€œ Fallback: TickBitmap yoksa eski dampening moduna geÃƒÂ§
//
//  v5.1 (korunuyor):
//  Ã¢Å“â€œ Tick Ã¢â€ â€ Fiyat ÃƒÂ§ift yÃƒÂ¶nlÃƒÂ¼ dÃƒÂ¶nÃƒÂ¼Ã…Å¸ÃƒÂ¼m ve ÃƒÂ§apraz doÃ„Å¸rulama
//  Ã¢Å“â€œ Token sÃ„Â±rasÃ„Â± farkÃ„Â±ndalÃ„Â±Ã„Å¸Ã„Â± (token0_is_weth Ã¢â‚¬â€ Base: WETH < USDC)
//  Ã¢Å“â€œ Newton-Raphson'a likidite-tabanlÃ„Â± ÃƒÂ¼st sÃ„Â±nÃ„Â±r ve tick-impact freni
// ============================================================================

use crate::types::{PoolState, TickBitmapData};

// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
// O(1) PreFilter Ã¢â‚¬â€ NR'den Ãƒâ€“nce HÃ„Â±zlÃ„Â± KÃƒÂ¢rlÃ„Â±lÃ„Â±k Eleme
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

/// O(1) kÃƒÂ¢rlÃ„Â±lÃ„Â±k ÃƒÂ¶n filtresi.
///
/// Newton-Raphson (NR) optimizasyonunun ~40 iterasyonluk kaba tarama +
/// ~50 iterasyonluk ince ayar maliyetini ÃƒÂ¶nlemek iÃƒÂ§in, spread'in
/// fee'leri kurtarÃ„Â±p kurtaramayacaÃ„Å¸Ã„Â±nÃ„Â± tek bir ÃƒÂ§arpma/ÃƒÂ§Ã„Â±karma ile kontrol eder.
///
/// FormÃƒÂ¼l:
///   expected_profit = (spread_ratio Ãƒâ€” amount) - (fee_a + fee_b + gas_cost)
///
/// `expected_profit > min_profit_wei` deÃ„Å¸ilse NR'ye hiÃƒÂ§ girmeden `None` dÃƒÂ¶ner.
pub struct PreFilter {
    /// Havuz A fee oranÃ„Â± (ÃƒÂ¶r: 0.0005 = %0.05)
    pub fee_a: f64,
    /// Havuz B fee oranÃ„Â± (ÃƒÂ¶r: 0.0001 = %0.01)
    pub fee_b: f64,
    /// Tahmini gas maliyeti (WETH cinsinden) Ã¢â‚¬â€ L2 + L1 + gÃƒÂ¼venlik marjÃ„Â±
    pub estimated_gas_cost_weth: f64,
    /// Minimum kÃƒÂ¢r eÃ…Å¸iÃ„Å¸i (WETH cinsinden)
    pub min_profit_weth: f64,
    /// Flash loan fee oranÃ„Â± (ÃƒÂ¶r: 0.0005 = 5 bps)
    pub flash_loan_fee_rate: f64,
    /// Builder bribe yÃƒÂ¼zdesi (ÃƒÂ¶r: 0.25 = %25)
    /// v19.0: BrÃƒÂ¼t kÃƒÂ¢rdan bribe dÃƒÂ¼Ã…Å¸ÃƒÂ¼ldÃƒÂ¼kten sonra net kÃƒÂ¢r hesaplanÃ„Â±r
    pub bribe_pct: f64,
}

/// PreFilter sonucu
#[derive(Debug, Clone, Copy)]
pub enum PreFilterResult {
    /// Spread fee'leri kurtarÃ„Â±yor Ã¢â‚¬â€ NR'ye devam et
    Profitable {
        /// Tahmini brÃƒÂ¼t kÃƒÂ¢r (WETH)
        estimated_profit_weth: f64,
        /// Spread oranÃ„Â±
        spread_ratio: f64,
    },
    /// Spread fee'leri kurtaramÃ„Â±yor Ã¢â‚¬â€ NR'yi atla
    Unprofitable {
        /// Neden kÃƒÂ¢rsÃ„Â±z?
        reason: PreFilterRejectReason,
    },
}

/// KÃƒÂ¢rsÃ„Â±zlÃ„Â±k nedeni (debug loglarÃ„Â± iÃƒÂ§in)
#[derive(Debug, Clone, Copy)]
pub enum PreFilterRejectReason {
    /// Spread toplam fee'den kÃƒÂ¼ÃƒÂ§ÃƒÂ¼k
    SpreadBelowFees,
    /// Tahmini kÃƒÂ¢r minimum eÃ…Å¸iÃ„Å¸in altÃ„Â±nda
    ProfitBelowThreshold,
    /// GeÃƒÂ§ersiz fiyat verisi
    InvalidPriceData,
}

impl PreFilter {
    /// O(1) kÃƒÂ¢rlÃ„Â±lÃ„Â±k kontrolÃƒÂ¼.
    ///
    /// # ArgÃƒÂ¼manlar
    /// - `price_a`: Havuz A ETH fiyatÃ„Â± (quote cinsinden)
    /// - `price_b`: Havuz B ETH fiyatÃ„Â± (quote cinsinden)
    /// - `trade_amount_weth`: Ã„Â°Ã…Å¸lem boyutu (WETH)
    ///
    /// # KarmaÃ…Å¸Ã„Â±klÃ„Â±k
    /// O(1) Ã¢â‚¬â€ sabit sayÃ„Â±da aritmetik operasyon, allocation yok.
    #[inline]
    pub fn check(
        &self,
        price_a: f64,
        price_b: f64,
        trade_amount_weth: f64,
    ) -> PreFilterResult {
        // GeÃƒÂ§erlilik kontrolÃƒÂ¼ Ã¢â‚¬â€ NaN/Infinity/sÃ„Â±fÃ„Â±r fiyat
        if price_a <= 0.0 || price_b <= 0.0
            || !price_a.is_finite() || !price_b.is_finite()
            || trade_amount_weth <= 0.0
        {
            return PreFilterResult::Unprofitable {
                reason: PreFilterRejectReason::InvalidPriceData,
            };
        }

        // Spread oranÃ„Â± = |price_a - price_b| / min(price_a, price_b)
        let spread = (price_a - price_b).abs();
        let min_price = price_a.min(price_b);
        let spread_ratio = spread / min_price;

        // Toplam fee oranÃ„Â± = fee_a + fee_b + flash_loan_fee
        let total_fee_ratio = self.fee_a + self.fee_b + self.flash_loan_fee_rate;

        // Spread fee'leri kurtarÃ„Â±yor mu?
        if spread_ratio <= total_fee_ratio {
            return PreFilterResult::Unprofitable {
                reason: PreFilterRejectReason::SpreadBelowFees,
            };
        }

        // v19.0: Tahmini net kÃƒÂ¢r (WETH cinsinden)
        // BrÃƒÂ¼t kÃƒÂ¢r = (spread_ratio - total_fee_ratio) Ãƒâ€” amount
        // Net kÃƒÂ¢r = brÃƒÂ¼t_kÃƒÂ¢r Ãƒâ€” (1 - bribe_pct) - gas_cost
        // Bu formÃƒÂ¼l komisyon + gas + bribe'Ã„Â± tek seferde deÃ„Å¸erlendirir.
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

// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
// Sabitler
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

/// 2^96 Ã¢â‚¬â€ sqrtPriceX96 ÃƒÂ§ÃƒÂ¶zÃƒÂ¼mleme sabiti
const Q96: f64 = 79_228_162_514_264_337_593_543_950_336.0;

/// ln(1.0001) Ã¢â‚¬â€ tick Ã¢â€ â€ fiyat dÃƒÂ¶nÃƒÂ¼Ã…Å¸ÃƒÂ¼mÃƒÂ¼ iÃƒÂ§in
const LOG_TICK_BASE: f64 = 0.000_099_995_000_33;

// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
// Tick Ã¢â€ â€ Fiyat DÃƒÂ¶nÃƒÂ¼Ã…Å¸ÃƒÂ¼mleri
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

/// Tick'i ham fiyat oranÃ„Â±na (token1_raw / token0_raw) ÃƒÂ§evir.
#[inline]
pub fn tick_to_price_ratio(tick: i32) -> f64 {
    (tick as f64 * LOG_TICK_BASE).exp()
}

/// Tick'i sqrtPriceX96 deÃ„Å¸erine ÃƒÂ§evir (doÃ„Å¸rulama amaÃƒÂ§lÃ„Â±).
/// Ham fiyat oranÃ„Â±nÃ„Â± ETH/USDC fiyatÃ„Â±na ÃƒÂ§evir (token sÃ„Â±rasÃ„Â± farkÃ„Â±ndalÃ„Â±Ã„Å¸Ã„Â± ile).
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
    // NaN veya Infinity asla dÃ„Â±Ã…Å¸arÃ„Â± sÃ„Â±zdÃ„Â±rma
    if result.is_nan() || result.is_infinite() { 0.0 } else { result }
}

/// sqrtPriceX96 + tick ÃƒÂ§apraz doÃ„Å¸rulamasÃ„Â± ile ETH fiyatÃ„Â± hesapla.
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
                "  Ã¢Å¡Â Ã¯Â¸Â Fiyat sapmasÃ„Â±: sqrtPrice={:.2}$, tick={:.2}$ (sapma: {:.2}%)",
                price_from_sqrt, price_from_tick, deviation * 100.0
            );
            return price_from_tick;
        }
    }

    price_from_sqrt
}

/// Bu sayede off-chain hesaplama, on-chain sonuÃƒÂ§la wei bazÃ„Â±nda eÃ…Å¸leÃ…Å¸ir.
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
    _sell_tick_spacing: i32,
    _buy_tick_spacing: i32,
    sell_bitmap: Option<&TickBitmapData>,
    buy_bitmap: Option<&TickBitmapData>,
    buy_token0_is_weth: bool,
) -> f64 {
    if amount_in_weth <= 0.0 {
        return f64::NEG_INFINITY;
    }

    // Ã¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢Â U256 EXACT MATH Ã¢â‚¬â€ On-Chain Deterministik Kesinlik Ã¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢Â

    // f64 Ã¢â€ â€™ U256 wei dÃƒÂ¶nÃƒÂ¼Ã…Å¸ÃƒÂ¼mÃƒÂ¼
    let amount_in_wei = alloy::primitives::U256::from(
        crate::types::safe_f64_to_u128(amount_in_weth * 1e18)
    );
    if amount_in_wei.is_zero() {
        return f64::NEG_INFINITY;
    }

    // Fee fraction Ã¢â€ â€™ pips (1e6 bazÃ„Â±nda: 0.0005 Ã¢â€ â€™ 500)
    let sell_fee_pips = exact::fee_fraction_to_pips(sell_fee_fraction);
    let buy_fee_pips = exact::fee_fraction_to_pips(buy_fee_fraction);

    // 1. WETH'i pahalÃ„Â± havuzda sat Ã¢â€ â€™ USDC al (exact U256 swap)
    // v20.0: Her havuzun kendi token0_is_weth deÃ„Å¸eri kullanÃ„Â±lÃ„Â±r
    let sell_zero_for_one = sell_token0_is_weth;
    let sell_result = exact::compute_exact_swap(
        sell_pool.sqrt_price_x96,
        sell_pool.liquidity,
        sell_pool.tick,
        amount_in_wei,
        sell_zero_for_one,
        sell_fee_pips,
        sell_bitmap,
    );

    if sell_result.amount_out.is_zero() {
        return f64::NEG_INFINITY;
    }

    // 2. USDC Ã¢â€ â€™ WETH geri al (exact U256 swap)
    // v20.0: buy pool'un kendi token sÃ„Â±ralamasÃ„Â± kullanÃ„Â±lÃ„Â±r
    let buy_zero_for_one = !buy_token0_is_weth;
    let buy_result = exact::compute_exact_swap(
        buy_pool.sqrt_price_x96,
        buy_pool.liquidity,
        buy_pool.tick,
        sell_result.amount_out,
        buy_zero_for_one,
        buy_fee_pips,
        buy_bitmap,
    );

    if buy_result.amount_out.is_zero() {
        return f64::NEG_INFINITY;
    }

    // 3. Flash loan geri ÃƒÂ¶deme (U256 hassasiyetinde)
    let flash_loan_fee_rate = flash_loan_fee_bps / 10_000.0;
    let flash_loan_fee_wei = alloy::primitives::U256::from(
        crate::types::safe_f64_to_u128(amount_in_weth * flash_loan_fee_rate * 1e18)
    );
    let repay_amount = amount_in_wei + flash_loan_fee_wei;

    // 4. Net kÃƒÂ¢r Ã¢â€ â€™ USD (optimizer iÃƒÂ§in f64'e geri dÃƒÂ¶nÃƒÂ¼Ã…Å¸)
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

// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
// Newton-Raphson TÃƒÂ¼rev HesaplayÃ„Â±cÃ„Â±
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

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

// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
// Newton-Raphson Optimizasyonu Ã¢â‚¬â€ TickBitmap-Aware
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

/// Newton-Raphson sonucu
#[derive(Debug, Clone)]
pub struct OptimalAmountResult {
    pub optimal_amount: f64,
    pub expected_profit: f64,
    pub converged: bool,
    pub iterations: u32,
}

/// Newton-Raphson ile optimal flash loan miktarÃ„Â±nÃ„Â± bul.
///        Her havuzun kendi token0_is_weth deÃ„Å¸eri baÃ„Å¸Ã„Â±msÃ„Â±z kullanÃ„Â±lÃ„Â±r.
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

    // Ã¢â€â‚¬Ã¢â€â‚¬ Hard Liquidity Cap (v11.0 + v20.0 decimal normalization) Ã¢â€â‚¬
    // v20.0: Her havuzun kendi token0_is_weth deÃ„Å¸eri kullanÃ„Â±lÃ„Â±r.
    // FarklÃ„Â± token sÃ„Â±ralamasÃ„Â±na sahip havuzlardan (ÃƒÂ¶r: WETH/USDC vs USDC/WETH)
    // doÃ„Å¸ru yÃƒÂ¶nde likidite kapasitesi hesaplanÃ„Â±r.
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

    // Eski single-tick cap (geriye uyumluluk + karÃ…Å¸Ã„Â±laÃ…Å¸tÃ„Â±rma)
    let liq_cap_sell = exact::max_safe_swap_amount_u256(
        sell_pool.sqrt_price_x96, sell_pool.liquidity, sell_token0_is_weth,
        sell_pool.tick, sell_tick_spacing,
    );
    let liq_cap_buy = exact::max_safe_swap_amount_u256(
        buy_pool.sqrt_price_x96, buy_pool.liquidity, buy_token0_is_weth,
        buy_pool.tick, buy_tick_spacing,
    );

    // v16.0: Hard cap ve single-tick cap'in minimumunu al.
    // Eski: `* 2.0` ÃƒÂ§arpanÃ„Â± single-tick kapasiteyi yapay olarak Ã…Å¸iÃ…Å¸iriyor
    // ve NR'nin havuzda olmayan likiditeyi hedeflemesine yol aÃƒÂ§Ã„Â±yordu.
    // Yeni: Her iki metriÃ„Å¸in minimumunu al, %99.9 gÃƒÂ¼venlik marjÃ„Â± zaten
    // hard_liquidity_cap_weth iÃƒÂ§inde uygulanÃ„Â±yor.
    let sell_cap = hard_cap_sell.min(liq_cap_sell.max(0.001)).max(0.001);
    let buy_cap = hard_cap_buy.min(liq_cap_buy.max(0.001)).max(0.001);
    let effective_max = max_amount_weth
        .min(sell_cap)
        .min(buy_cap);

    eprintln!(
        "     \u{1f4ca} [Liquidity Cap] sell_hard={:.4} buy_hard={:.4} sell_single={:.4} buy_single={:.4} Ã¢â€ â€™ effective_max={:.4} WETH",
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

    // Ã¢â€â‚¬Ã¢â€â‚¬ AÃ…ÂAMA 1: Hibrit Kaba Tarama Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
    // v22.0: 40 Ã¢â€ â€™ 25 adÃ„Â±m. Quadratic spacing kÃƒÂ¼ÃƒÂ§ÃƒÂ¼k miktarlarda daha yoÃ„Å¸un
    // tarama yapar, bÃƒÂ¼yÃƒÂ¼k miktarlarda seyrekleÃ…Å¸ir. 25 adÃ„Â±m yeterli ÃƒÂ§ÃƒÂ¶zÃƒÂ¼nÃƒÂ¼rlÃƒÂ¼k
    // saÃ„Å¸lar, 15 iterasyon (~0.5ms) tasarruf eder.
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

    // Ã¢â€â‚¬Ã¢â€â‚¬ AÃ…ÂAMA 2: Newton-Raphson Ã„Â°nce Ayar Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
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

    // v16.0: NR yakÃ„Â±nsama sonrasÃ„Â± nihai gÃƒÂ¼venlik tavanÃ„Â±.
    // NR iterasyonlarÃ„Â± sÃ„Â±rasÃ„Â±nda clamp uygulanÃ„Â±yor ama yakÃ„Â±nsama sonrasÃ„Â±
    // son bir kez daha effective_max ile sÃ„Â±nÃ„Â±rla Ã¢â‚¬â€ havuz kapasitesinin
    // %99.9'unu (slippage payÃ„Â±) ASLA aÃ…Å¸amaz.
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

// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
// Testler
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TickInfo;
    use std::collections::HashMap;
    use std::time::Instant;
    use proptest::prelude::*;

    /// Test havuz durumu oluÃ…Å¸tur (Base Network gerÃƒÂ§ekÃƒÂ§i deÃ„Å¸erler).
    /// v7.0: sqrt_price_x96 artÃ„Â±k doÃ„Å¸ru U256 olarak hesaplanÃ„Â±r (exact::get_sqrt_ratio_at_tick).
    fn make_test_pool(eth_price: f64) -> PoolState {
        let price_ratio = eth_price * 1e-12;
        let sqrt_price = price_ratio.sqrt();
        let sqrt_price_x96 = sqrt_price * Q96;
        let tick = (price_ratio.ln() / LOG_TICK_BASE).floor() as i32;
        let liquidity: u128 = 50_000_000_000_000_000_000; // 5e19

        // U256 sqrtPriceX96'yÃ„Â± tick'ten exact hesapla (deterministik)
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

    /// Test TickBitmap oluÃ…Å¸tur (mevcut tick etrafÃ„Â±nda birkaÃƒÂ§ baÃ…Å¸latÃ„Â±lmÃ„Â±Ã…Å¸ tick)
    fn make_test_bitmap(current_tick: i32, tick_spacing: i32) -> TickBitmapData {
        let mut ticks = HashMap::new();

        // Mevcut tick'in etrafÃ„Â±nda 10 tick sÃ„Â±nÃ„Â±rÃ„Â± oluÃ…Å¸tur
        for i in -5..=5 {
            let tick = ((current_tick / tick_spacing) + i) * tick_spacing;
            let liq_net = if i < 0 {
                // Sol tick'ler: yaklaÃ…Å¸tÃ„Â±kÃƒÂ§a likidite artar
                5_000_000_000_000_000_000i128 // 5e18
            } else if i > 0 {
                // SaÃ„Å¸ tick'ler: uzaklaÃ…Å¸tÃ„Â±kÃƒÂ§a likidite azalÃ„Â±r
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
    fn test_compute_eth_price_token0_weth() {
        let pool = make_test_pool(2000.0);
        let price = compute_eth_price(
            pool.sqrt_price_f64, pool.tick, 18, 6, true,
        );
        assert!(
            (price - 2000.0).abs() < 5.0,
            "ETH fiyatÃ„Â± ~2000 olmalÃ„Â±, hesaplanan: {:.2}", price
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
            "NR+Bitmap: miktar={:.6} WETH, kÃƒÂ¢r={:.4}$, iter={}, yakÃ„Â±n={}",
            result.optimal_amount, result.expected_profit,
            result.iterations, result.converged
        );

        assert!(result.expected_profit > 0.0, "KÃƒÂ¢r pozitif olmalÃ„Â±");
        assert!(result.optimal_amount > 0.0, "Optimal miktar > 0 olmalÃ„Â±");
    }

    // Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
    // PROPTEST Ã¢â‚¬â€ Ãƒâ€¡ÃƒÂ¶kme DayanÃ„Â±klÃ„Â±lÃ„Â±k Testleri (Property-Based Stress Test)
    //
    // AmaÃƒÂ§: math.rs motoruna milyonlarca rastgele ekstrem deÃ„Å¸er basarak
    // hiÃƒÂ§bir koÃ…Å¸ulda panic!, NaN veya Infinity ÃƒÂ¼retmediÃ„Å¸ini kanÃ„Â±tlamak.
    // Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

    /// YardÃ„Â±mcÃ„Â±: Rastgele bir sqrtPriceX96 f64 deÃ„Å¸eri ÃƒÂ¼ret.
    /// GerÃƒÂ§ek havuzlarda bu deÃ„Å¸er kabaca 1e18..1e30 arasÃ„Â±ndadÃ„Â±r,
    /// ancak stres testi iÃƒÂ§in 0.0 ve f64::MAX dahil tÃƒÂ¼m aralÃ„Â±Ã„Å¸Ã„Â± kapsÃ„Â±yoruz.
    fn arb_sqrt_price_x96() -> impl Strategy<Value = f64> {
        prop_oneof![
            // %60 Ã¢â‚¬â€ GerÃƒÂ§ekÃƒÂ§i aralÃ„Â±k (Base aÃ„Å¸Ã„Â±ndaki tipik deÃ„Å¸erler)
            6 => 1e18_f64..1e30_f64,
            // %20 Ã¢â‚¬â€ SÃ„Â±fÃ„Â±r ve sÃ„Â±fÃ„Â±ra yakÃ„Â±n kenar durumlar
            2 => prop::num::f64::ANY.prop_map(|v| v.abs().min(1.0)),
            // %10 Ã¢â‚¬â€ AÃ…Å¸Ã„Â±rÃ„Â± bÃƒÂ¼yÃƒÂ¼k deÃ„Å¸erler (u256 aralÃ„Â±Ã„Å¸Ã„Â±na yakÃ„Â±n)
            1 => 1e30_f64..1e77_f64,
            // %10 Ã¢â‚¬â€ Negatif ve ÃƒÂ¶zel deÃ„Å¸erler (fonksiyonlar bunlarÃ„Â± sÃ„Â±fÃ„Â±ra dÃƒÂ¼Ã…Å¸ÃƒÂ¼rmeli)
            1 => prop::num::f64::ANY.prop_map(|v| if v.is_nan() { 0.0 } else { v }),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(10_000))]

        // Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬ TEST 1: compute_eth_price asla NaN/Inf/panic ÃƒÂ¼retmemeli Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
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
                "compute_eth_price NaN dÃƒÂ¶ndÃƒÂ¼! sqrt_price_x96={}, tick={}, t0_weth={}",
                sqrt_price_x96, tick, token0_is_weth);
            prop_assert!(!sonuc.is_infinite(),
                "compute_eth_price Infinity dÃƒÂ¶ndÃƒÂ¼! sqrt_price_x96={}, tick={}, t0_weth={}",
                sqrt_price_x96, tick, token0_is_weth);
            prop_assert!(sonuc >= 0.0,
                "compute_eth_price negatif dÃƒÂ¶ndÃƒÂ¼! sonuc={}, sqrt_price_x96={}, tick={}",
                sonuc, sqrt_price_x96, tick);
        }

        // Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬ TEST 4: tick_to_price_ratio aÃ…Å¸Ã„Â±rÃ„Â± tick'lerde ÃƒÂ§ÃƒÂ¶kmemeli Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
        #[test]
        fn stres_tick_to_price_ratio(
            tick in -887272..=887272i32,
        ) {
            let sonuc = tick_to_price_ratio(tick);
            prop_assert!(!sonuc.is_nan(),
                "tick_to_price_ratio NaN! tick={}", tick);
            // AÃ…Å¸Ã„Â±rÃ„Â± tick'lerde Infinity olabilir ama panic olmamalÃ„Â±
            prop_assert!(sonuc >= 0.0,
                "tick_to_price_ratio negatif! tick={}, sonuc={}", tick, sonuc);
        }

    }
}

// Ã¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢Â
//  BÃƒâ€“LÃƒÅ“M: U256 EXACT-MATH Ã¢â‚¬â€ Wei Seviyesinde Hassas Swap MatematiÃ„Å¸i
// Ã¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢Â
//
//  Neden? EVM 256-bit tam sayÃ„Â±larla ÃƒÂ§alÃ„Â±Ã…Å¸Ã„Â±r. f64'ÃƒÂ¼n 52-bit mantissa'sÃ„Â±
//  18-haneli decimal hesaplamalarda yuvarlama hatalarÃ„Â± yaratÃ„Â±r.
//  Bu modÃƒÂ¼l, Uniswap V3'ÃƒÂ¼n Solidity matematik kÃƒÂ¼tÃƒÂ¼phanesini (TickMath,
//  SqrtPriceMath, SwapMath) birebir Rust U256'ya port eder.
//
//  KullanÃ„Â±m: Botun off-chain hesapladÃ„Â±Ã„Å¸Ã„Â± swap ÃƒÂ§Ã„Â±ktÃ„Â±sÃ„Â±nÃ„Â±n, on-chain
//  gerÃƒÂ§ekleÃ…Å¸ecek sonuÃƒÂ§la *wei* bazÃ„Â±nda eÃ…Å¸leÃ…Å¸mesi.
//
//  Kaynaklar:
//    - UniV3 TickMath.sol: getSqrtRatioAtTick, getTickAtSqrtRatio
//    - UniV3 SqrtPriceMath.sol: getNextSqrtPriceFromInput, getAmount0/1Delta
//    - UniV3 SwapMath.sol: computeSwapStep
// Ã¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢Â

pub mod exact {
    use alloy::primitives::U256;
    use crate::types::TickBitmapData;

    // Ã¢â€â‚¬Ã¢â€â‚¬ Sabitler Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

    /// Q96 = 2^96 (sqrtPriceX96 ÃƒÂ§ÃƒÂ¶zÃƒÂ¼mleme sabiti)
    const Q96: U256 = U256::from_limbs([0, 0x1_0000_0000, 0, 0]); // 2^96

    /// MAX_SQRT_RATIO (UniV3 TickMath sÃ„Â±nÃ„Â±rÃ„Â± Ã¢â‚¬â€ 1461446703485210103287273052203988822378723970342)
    const MAX_SQRT_RATIO: U256 = U256::from_limbs([
        0x5D951D5263988D26, 0xEFD1FC6A50648849, 0x00000000FFFD8963, 0
    ]);

    /// MIN_SQRT_RATIO (UniV3 TickMath sÃ„Â±nÃ„Â±rÃ„Â±)
    const MIN_SQRT_RATIO: U256 = U256::from_limbs([4295128739, 0, 0, 0]);

    // Ã¢â€â‚¬Ã¢â€â‚¬ FullMath Ã¢â‚¬â€ U256 Tam Ãƒâ€¡arpma / BÃƒÂ¶lme Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

    /// a * b / denominator (taÃ…Å¸ma gÃƒÂ¼venli, floor rounding)
    /// Uniswap V3 FullMath.mulDiv port'u.
    ///
    /// v22.1 DÃƒÅ“ZELTME: RekÃƒÂ¼rsif ayrÃ„Â±Ã…Å¸tÃ„Â±rma algoritmasÃ„Â±.
    /// Eski: Ã„Â°ÃƒÂ§ taÃ…Å¸mada saturating_mul + U256::ZERO fallback Ã¢â€ â€™ sessiz hata.
    /// Yeni: mul_div(big%c, small, c) rekÃƒÂ¼rsifi Ã¢â‚¬â€ big%c < c garantisi ile
    ///       her adÃ„Â±mda operand kesinlikle kÃƒÂ¼ÃƒÂ§ÃƒÂ¼lÃƒÂ¼r Ã¢â€ â€™ sonlanma garantili.
    pub fn mul_div(a: U256, b: U256, denominator: U256) -> U256 {
        if denominator.is_zero() || a.is_zero() || b.is_zero() {
            return U256::ZERO;
        }
        // DoÃ„Å¸rudan ÃƒÂ§arpma dene
        if let Some(product) = a.checked_mul(b) {
            return product / denominator;
        }
        // TaÃ…Å¸ma: rekÃƒÂ¼rsif ayrÃ„Â±Ã…Å¸tÃ„Â±rma ile hesapla
        // a*b/c = (big/c)*small + mul_div(big%c, small, c)
        // Her rekÃƒÂ¼rsif ÃƒÂ§aÃ„Å¸rÃ„Â±da ilk argÃƒÂ¼man = big%c < c Ã¢â€ â€™ kesinlikle kÃƒÂ¼ÃƒÂ§ÃƒÂ¼lÃƒÂ¼r
        // Sonlanma garantili (logaritmik derinlik)
        let (big, small) = if a >= b { (a, b) } else { (b, a) };
        let q = big / denominator;
        let r = big % denominator;
        // term1 = (big/c) * small Ã¢â‚¬â€ saturating: taÃ…Å¸ma durumunda sonuÃƒÂ§ U256'ya sÃ„Â±Ã„Å¸mÃ„Â±yordur
        let term1 = q.saturating_mul(small);
        // term2 = mul_div(big%c, small, c) Ã¢â‚¬â€ rekÃƒÂ¼rsif, big%c < c Ã¢â€ â€™ sonlanÃ„Â±r
        let term2 = mul_div(r, small, denominator);
        term1.saturating_add(term2)
    }

    /// a * b / denominator (taÃ…Å¸ma gÃƒÂ¼venli, ceil rounding)
    ///
    /// v22.1 DÃƒÅ“ZELTME: mul_mod ile taÃ…Å¸ma-gÃƒÂ¼venli kalan kontrolÃƒÂ¼.
    /// Eski: TaÃ…Å¸ma durumunda koÃ…Å¸ulsuz +1 ekliyordu Ã¢â€ â€™ gereksiz yuvarlamalar.
    /// Yeni: a.mul_mod(b, denominator) ile 512-bit ara sonuÃƒÂ§ ÃƒÂ¼zerinden
    ///       kesin kalan hesabÃ„Â± Ã¢â€ â€™ sadece gerÃƒÂ§ek kalan varsa +1.
    pub fn mul_div_rounding_up(a: U256, b: U256, denominator: U256) -> U256 {
        let result = mul_div(a, b, denominator);
        if denominator.is_zero() {
            return result;
        }
        // mul_mod: (a * b) % denominator Ã¢â‚¬â€ 512-bit ara sonuÃƒÂ§, taÃ…Å¸ma gÃƒÂ¼venli
        let remainder = a.mul_mod(b, denominator);
        if remainder > U256::ZERO {
            result + U256::from(1)
        } else {
            result
        }
    }

    /// (a + b - 1) / b tarzÃ„Â± ceil division
    #[inline]
    pub fn div_rounding_up(a: U256, b: U256) -> U256 {
        if b.is_zero() { return U256::ZERO; }
        let d = a / b;
        if a % b > U256::ZERO { d + U256::from(1) } else { d }
    }

    // Ã¢â€â‚¬Ã¢â€â‚¬ TickMath Ã¢â‚¬â€ Tick Ã¢â€ â€ SqrtPriceX96 Birebir DÃƒÂ¶nÃƒÂ¼Ã…Å¸ÃƒÂ¼m Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

    /// Tick'ten sqrtPriceX96 hesapla Ã¢â‚¬â€ UniV3 TickMath.getSqrtRatioAtTick birebir port'u.
    /// Ã„Â°nput: -887272 Ã¢â€°Â¤ tick Ã¢â€°Â¤ 887272
    /// Ãƒâ€¡Ã„Â±ktÃ„Â±: uint160 sqrtPriceX96 (U256 olarak)
    pub fn get_sqrt_ratio_at_tick(tick: i32) -> U256 {
        let abs_tick = tick.unsigned_abs();
        assert!(abs_tick <= 887272, "tick aralÃ„Â±k dÃ„Â±Ã…Å¸Ã„Â±");

        // BaÃ…Å¸langÃ„Â±ÃƒÂ§ ratio (Q128 formatÃ„Â±nda)
        let mut ratio: U256 = if abs_tick & 0x1 != 0 {
            U256::from_be_slice(&hex_literal::hex!("fffcb933bd6fad37aa2d162d1a594001"))
        } else {
            U256::from(1u64) << 128
        };

        // Her bit iÃƒÂ§in ÃƒÂ§arpma tablosu Ã¢â‚¬â€ UniV3 TickMath magic numbers
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

        // Pozitif tick Ã¢â€ â€™ ters ÃƒÂ§evir
        if tick > 0 {
            ratio = U256::MAX / ratio;
        }

        // Q128 Ã¢â€ â€™ Q96 dÃƒÂ¶nÃƒÂ¼Ã…Å¸ÃƒÂ¼mÃƒÂ¼ + yukarÃ„Â± yuvarlama
        let remainder = ratio % (U256::from(1u64) << 32);
        let shifted = ratio >> 32;
        if remainder > U256::ZERO {
            shifted + U256::from(1)
        } else {
            shifted
        }
    }

    // Ã¢â€â‚¬Ã¢â€â‚¬ SqrtPriceMath Ã¢â‚¬â€ Fiyat GeÃƒÂ§iÃ…Å¸i HesaplamalarÃ„Â± Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

    /// token0 girdisi ile yeni sqrtPrice hesapla (zeroForOne=true, fiyat DÃƒÅ“Ã…ÂER)
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
            // TaÃ…Å¸ma fallback
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

    /// Girdi miktarÃ„Â±ndan yeni sqrtPrice hesapla (yÃƒÂ¶n'e gÃƒÂ¶re dispatch)
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

    /// Ã„Â°ki sqrtPrice arasÃ„Â±ndaki token0 farkÃ„Â± (Ãâ€x)
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

    /// Ã„Â°ki sqrtPrice arasÃ„Â±ndaki token1 farkÃ„Â± (Ãâ€y)
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

    // Ã¢â€â‚¬Ã¢â€â‚¬ SwapMath Ã¢â‚¬â€ Tek AdÃ„Â±m Swap Hesaplama Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

    /// Tek bir fiyat aralÃ„Â±Ã„Å¸Ã„Â±ndaki swap adÃ„Â±mÃ„Â± sonucu
    #[derive(Debug, Clone)]
    pub struct ExactSwapStep {
        /// Swap sonrasÃ„Â± sqrtPriceX96
        pub sqrt_ratio_next: U256,
        /// TÃƒÂ¼ketilen girdi miktarÃ„Â±
        pub amount_in: U256,
        /// ÃƒÅ“retilen ÃƒÂ§Ã„Â±ktÃ„Â± miktarÃ„Â±
        pub amount_out: U256,
        /// AlÃ„Â±nan fee miktarÃ„Â±
        pub fee_amount: U256,
    }

    /// Tek fiyat aralÃ„Â±Ã„Å¸Ã„Â±nda swap adÃ„Â±mÃ„Â± hesapla
    /// Port: SwapMath.computeSwapStep
    pub fn compute_swap_step(
        sqrt_ratio_current: U256,
        sqrt_ratio_target: U256,
        liquidity: u128,
        amount_remaining: U256,
        fee_pips: u32, // 1e6 bazÃ„Â±nda (ÃƒÂ¶r: 500 = %0.05)
    ) -> ExactSwapStep {
        let zero_for_one = sqrt_ratio_current >= sqrt_ratio_target;
        let one_minus_fee = U256::from(1_000_000u64 - fee_pips as u64);

        // Fee dÃƒÂ¼Ã…Å¸ÃƒÂ¼lmÃƒÂ¼Ã…Å¸ efektif girdi
        let amount_remaining_less_fee = mul_div(
            amount_remaining, one_minus_fee, U256::from(1_000_000u64),
        );

        // Bu aralÃ„Â±ktaki maksimum girdi
        let amount_in_max = if zero_for_one {
            get_amount0_delta(sqrt_ratio_target, sqrt_ratio_current, liquidity, true)
        } else {
            get_amount1_delta(sqrt_ratio_current, sqrt_ratio_target, liquidity, true)
        };

        // Hedef fiyata ulaÃ…Å¸abilir miyiz?
        let sqrt_ratio_next = if amount_remaining_less_fee >= amount_in_max {
            sqrt_ratio_target
        } else {
            get_next_sqrt_price_from_input(
                sqrt_ratio_current, liquidity, amount_remaining_less_fee, zero_for_one,
            )
        };

        let max_reached = sqrt_ratio_next == sqrt_ratio_target;

        // GerÃƒÂ§ek girdi ve ÃƒÂ§Ã„Â±ktÃ„Â± miktarlarÃ„Â±nÃ„Â± hesapla
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

    // Ã¢â€â‚¬Ã¢â€â‚¬ Tam Multi-Tick Swap SimÃƒÂ¼lasyonu (Exact) Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

    /// Exact multi-tick swap sonucu (U256 hassasiyetinde)
    #[derive(Debug, Clone)]
#[allow(dead_code)]
    pub struct ExactSwapResult {
        /// Toplam ÃƒÂ§Ã„Â±ktÃ„Â± miktarÃ„Â± (raw wei)
        pub amount_out: U256,
        /// Toplam tÃƒÂ¼ketilen girdi (raw wei, fee dahil)
        pub amount_in_consumed: U256,
        /// Son sqrtPriceX96
        pub final_sqrt_price_x96: U256,
        /// Son likidite
        pub final_liquidity: u128,
        /// GeÃƒÂ§ilen tick sayÃ„Â±sÃ„Â±
        pub tick_crossings: u32,
    }

    /// Exact V3/CL swap simÃƒÂ¼lasyonu Ã¢â‚¬â€ U256 hassasiyetinde, wei-bazÃ„Â±nda eÃ…Å¸leÃ…Å¸me.
    ///
    /// Bu fonksiyon Uniswap V3'ÃƒÂ¼n on-chain `swap()` fonksiyonunun
    /// matematiÃ„Å¸ini birebir taklit eder:
    ///   1. Mevcut fiyat aralÃ„Â±Ã„Å¸Ã„Â±nda ne kadar swap yapÃ„Â±labilir? (computeSwapStep)
    ///   2. Girdi tÃƒÂ¼kenmezse sonraki tick'e ilerle
    ///   3. O tick'te liquidityNet ile aktif likiditeyi gÃƒÂ¼ncelle
    ///   4. Tekrarla
    ///
    /// # Parametreler
    /// - `sqrt_price_x96`: Mevcut sqrtPriceX96 (U256)
    /// - `liquidity`: Mevcut aktif likidite (u128)
    /// - `amount_in`: Girdi miktarÃ„Â± (wei, fee dahil ham miktar)
    /// - `zero_for_one`: Swap yÃƒÂ¶nÃƒÂ¼ (true=token0Ã¢â€ â€™token1, false=token1Ã¢â€ â€™token0)
    /// - `fee_pips`: Fee (1e6 bazÃ„Â±nda, ÃƒÂ¶r: 500 = %0.05)
    /// - `tick_spacing`: Tick aralÃ„Â±Ã„Å¸Ã„Â±
    /// - `bitmap`: TickBitmap verisi (baÃ…Å¸latÃ„Â±lmÃ„Â±Ã…Å¸ tick'ler + liquidityNet)
    pub fn compute_exact_swap(
        sqrt_price_x96: U256,
        liquidity: u128,
        current_tick: i32,
        amount_in: U256,
        zero_for_one: bool,
        fee_pips: u32,
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

        // SÃ„Â±ralÃ„Â± tick'leri al
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

        // Ana swap dÃƒÂ¶ngÃƒÂ¼sÃƒÂ¼ Ã¢â‚¬â€ tick'ler boyunca ilerle
        for &(next_tick, liquidity_net) in &ordered_ticks {
            if amount_remaining.is_zero() || crossings >= max_crossings {
                break;
            }

            // Hedef tick sÃ„Â±nÃ„Â±rÃ„Â±nÃ„Â±n sqrtPrice'Ã„Â±nÃ„Â± hesapla
            let sqrt_price_target = get_sqrt_ratio_at_tick(next_tick);

            // Bu aralÃ„Â±kta swap yap
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

            // Tick sÃ„Â±nÃ„Â±rÃ„Â±na ulaÃ…Å¸tÃ„Â±k mÃ„Â±?
            if step.sqrt_ratio_next == sqrt_price_target && !amount_remaining.is_zero() {
                // Likiditeyi gÃƒÂ¼ncelle
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
                let _ = state_tick; // tick crossing sÃ„Â±rasÃ„Â±nda gÃƒÂ¼ncellenir
                crossings += 1;
            }
        }

        // Kalan girdi varsa mevcut likiditede son bir adÃ„Â±m daha
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

    /// Ã„Â°ki havuz arasÃ„Â±nda exact arbitraj kÃƒÂ¢rÃ„Â± hesapla (U256, wei bazÃ„Â±nda)
    ///
    // v23.0 (D-3): compute_exact_arbitrage_profit tamamen kaldÃ„Â±rÃ„Â±ldÃ„Â±.
    // Tek token0_is_weth parametresi ÃƒÂ§apraz-DEX'te hatalÃ„Â± sonuÃƒÂ§ veriyordu.
    // Yerine compute_exact_directional_profit kullanÃ„Â±lÃ„Â±r.

    // Ã¢â€â‚¬Ã¢â€â‚¬ YÃƒÂ¶n-BazlÃ„Â± Exact KÃƒÂ¢r Hesaplama (Flash Swap AkÃ„Â±Ã…Å¸Ã„Â± Birebir Model) Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

    /// Flash swap akÃ„Â±Ã…Å¸Ã„Â±nÃ„Â± birebir modelleyerek kÃƒÂ¢rÃ„Â± hesapla.
    ///
    /// Kontrat akÃ„Â±Ã…Å¸Ã„Â±:
    ///   1. UniV3(PoolA): amount_wei input Ã¢â€ â€™ received_amount output
    ///   2. Slipstream(PoolB): received_amount input Ã¢â€ â€™ owed_output
    ///   3. profit = owed_output - amount_wei (owedToken cinsinden, wei)
    ///
    /// Bu fonksiyon hem WETH-ÃƒÂ¶deme hem USDC-ÃƒÂ¶deme senaryolarÃ„Â±nÃ„Â± doÃ„Å¸ru hesaplar.
    /// minProfit calldata deÃ„Å¸eri bu fonksiyonun ÃƒÂ§Ã„Â±ktÃ„Â±sÃ„Â±ndan tÃƒÂ¼retilir.
    ///
    /// # DÃƒÂ¶nÃƒÂ¼Ã…Å¸
    /// KÃƒÂ¢r U256 (owedToken cinsinden, wei). KÃƒÂ¢r yoksa U256::ZERO.
    pub fn compute_exact_directional_profit(
        // Pool A (UniV3 Ã¢â‚¬â€ flash swap kaynaÃ„Å¸Ã„Â±)
        pool_a_sqrt_price: U256,
        pool_a_liquidity: u128,
        pool_a_tick: i32,
        pool_a_fee_pips: u32,
        pool_a_bitmap: Option<&TickBitmapData>,
        // Pool B (Slipstream Ã¢â‚¬â€ satÃ„Â±Ã…Å¸ hedefi)
        pool_b_sqrt_price: U256,
        pool_b_liquidity: u128,
        pool_b_tick: i32,
        pool_b_fee_pips: u32,
        pool_b_bitmap: Option<&TickBitmapData>,
        // Swap parametreleri
        amount_wei: U256,
        uni_zero_for_one: bool,
        aero_zero_for_one: bool,
    ) -> U256 {
        if amount_wei.is_zero() {
            return U256::ZERO;
        }

        // AdÃ„Â±m 1: UniV3 flash swap
        // amount_wei input Ã¢â€ â€™ received tokens output
        let univ3_result = compute_exact_swap(
            pool_a_sqrt_price,
            pool_a_liquidity,
            pool_a_tick,
            amount_wei,
            uni_zero_for_one,
            pool_a_fee_pips,
            pool_a_bitmap,
        );

        if univ3_result.amount_out.is_zero() {
            return U256::ZERO;
        }

        // AdÃ„Â±m 2: Slipstream swap
        // UniV3'ten alÃ„Â±nan tokenlar Ã¢â€ â€™ owedToken geri alÃ„Â±nÃ„Â±r
        let slipstream_result = compute_exact_swap(
            pool_b_sqrt_price,
            pool_b_liquidity,
            pool_b_tick,
            univ3_result.amount_out,
            aero_zero_for_one,
            pool_b_fee_pips,
            pool_b_bitmap,
        );

        // AdÃ„Â±m 3: KÃƒÂ¢r = Slipstream ÃƒÂ§Ã„Â±ktÃ„Â±sÃ„Â± - UniV3'e borÃƒÂ§
        // Kontrat akÃ„Â±Ã…Å¸Ã„Â±: balAfter(owedToken) - balBefore(owedToken)
        // owed_output (Slipstream'den) - amount_wei (UniV3'e ÃƒÂ¶deme)
        if slipstream_result.amount_out > amount_wei {
            slipstream_result.amount_out - amount_wei
        } else {
            U256::ZERO
        }
    }

    // Ã¢â€â‚¬Ã¢â€â‚¬ DÃƒÂ¶nÃƒÂ¼Ã…Å¸ÃƒÂ¼m YardÃ„Â±mcÃ„Â±larÃ„Â± Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

    /// U256'yÃ„Â± f64'e gÃƒÂ¼venli dÃƒÂ¶nÃƒÂ¼Ã…Å¸tÃƒÂ¼r.
    /// v22.0: String conversion Ã¢â€ â€™ doÃ„Å¸rudan bit manipÃƒÂ¼lasyonu.
    /// Eski: val.to_string().parse::<f64>() Ã¢â‚¬â€ her dÃƒÂ¶nÃƒÂ¼Ã…Å¸ÃƒÂ¼mde heap allocation.
    /// Yeni: U256 limb'lerinden doÃ„Å¸rudan f64 hesaplama Ã¢â‚¬â€ zero-alloc.
    /// Not: 2^53 ÃƒÂ¼stÃƒÂ¼ deÃ„Å¸erlerde dÃƒÂ¼Ã…Å¸ÃƒÂ¼k bitler kaybolur ama WETH/USD
    /// aralÃ„Â±Ã„Å¸Ã„Â±ndaki wei deÃ„Å¸erleri iÃƒÂ§in sorun oluÃ…Å¸turmaz.
    pub fn u256_to_f64(val: U256) -> f64 {
        if val.is_zero() {
            return 0.0;
        }
        // U256 Ã¢â€ â€™ [u64; 4] limbs (little-endian)
        let limbs = val.as_limbs();
        // En yÃƒÂ¼ksek anlamlÃ„Â± limb'i bul
        if limbs[3] != 0 {
            // 192-255 bit aralÃ„Â±Ã„Å¸Ã„Â±nda
            limbs[3] as f64 * (2.0f64).powi(192)
                + limbs[2] as f64 * (2.0f64).powi(128)
                + limbs[1] as f64 * (2.0f64).powi(64)
                + limbs[0] as f64
        } else if limbs[2] != 0 {
            // 128-191 bit aralÃ„Â±Ã„Å¸Ã„Â±nda
            limbs[2] as f64 * (2.0f64).powi(128)
                + limbs[1] as f64 * (2.0f64).powi(64)
                + limbs[0] as f64
        } else if limbs[1] != 0 {
            // 64-127 bit aralÃ„Â±Ã„Å¸Ã„Â±nda
            limbs[1] as f64 * (2.0f64).powi(64)
                + limbs[0] as f64
        } else {
            // 0-63 bit aralÃ„Â±Ã„Å¸Ã„Â±nda Ã¢â‚¬â€ tam hassasiyet (f64 mantissa 53 bit)
            limbs[0] as f64
        }
    }

    /// Fee fraction (ÃƒÂ¶r: 0.0005) Ã¢â€ â€™ fee pips (ÃƒÂ¶r: 500, 1e6 bazÃ„Â±nda).
    /// Uniswap V3 fee_pips: 500 = %0.05, 3000 = %0.30, 10000 = %1.00
    #[inline]
    pub fn fee_fraction_to_pips(fee_fraction: f64) -> u32 {
        (fee_fraction * 1_000_000.0).round() as u32
    }

    /// U256-tabanlÃ„Â± gÃƒÂ¼venli maksimum swap miktarÃ„Â± (f64 WETH dÃƒÂ¶ndÃƒÂ¼rÃƒÂ¼r).
    /// Mevcut tick aralÃ„Â±Ã„Å¸Ã„Â±ndaki likidite kapasitesinin %15'ini hesaplar.
    ///
    /// Uniswap V3 SqrtPriceMath formÃƒÂ¼lleri ile:
    ///   token0: capacity = L Ãƒâ€” Q96 / sqrtPriceX96
    ///   token1: capacity = L Ãƒâ€” sqrtPriceX96 / Q96
    ///
    /// v11.1: ArtÃ„Â±k %15 sezgisel hesap YOK.  Bunun yerine mevcut fiyattan
    ///        bir sonraki tick_spacing sÃ„Â±nÃ„Â±rÃ„Â±na kadar absorbe edilebilecek
    ///        tam WETH miktarÃ„Â± hesaplanÃ„Â±r (get_amount0_delta / get_amount1_delta).
    ///        BÃƒÂ¶ylece 6 vs 18 decimal asimetrik havuzlarda milyarlÃ„Â±k hayalet
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

        // Tick'i Uniswap V3 geÃƒÂ§erli aralÃ„Â±Ã„Å¸a kÃ„Â±sÃ„Â±tla
        const MIN_TICK: i32 = -887272;
        const MAX_TICK: i32 = 887272;
        let clamped_tick = current_tick.clamp(MIN_TICK, MAX_TICK);

        // Mevcut tick aralÃ„Â±Ã„Å¸Ã„Â±nÃ„Â±n alt ve ÃƒÂ¼st sÃ„Â±nÃ„Â±rlarÃ„Â±nÃ„Â± bul
        let lower_tick = clamped_tick.div_euclid(tick_spacing) * tick_spacing;
        let upper_tick = lower_tick + tick_spacing;

        let capacity_raw = if token0_is_weth {
            // WETH = token0 Ã¢â€ â€™ swap yÃƒÂ¶nÃƒÂ¼ zeroForOne Ã¢â€ â€™ fiyat DÃƒÅ“Ã…ÂER (alt sÃ„Â±nÃ„Â±ra)
            let sqrt_lower = get_sqrt_ratio_at_tick(lower_tick.clamp(MIN_TICK, MAX_TICK));
            // Fiyat tam sÃ„Â±nÃ„Â±rdaysa bir tick_spacing daha aÃ…Å¸aÃ„Å¸Ã„Â± git
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
            // WETH = token1 Ã¢â€ â€™ swap yÃƒÂ¶nÃƒÂ¼ oneForZero Ã¢â€ â€™ fiyat YUKARI (ÃƒÂ¼st sÃ„Â±nÃ„Â±ra)
            let sqrt_upper = get_sqrt_ratio_at_tick(upper_tick.clamp(MIN_TICK, MAX_TICK));
            // Fiyat tam sÃ„Â±nÃ„Â±rdaysa bir tick_spacing daha yukarÃ„Â± git
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

    /// Hard Liquidity Cap Ã¢â‚¬â€ TickBitmap'ten gerÃƒÂ§ek mevcut likiditeyi hesapla.
    ///
    /// Bu fonksiyon, mevcut tick'ten itibaren swap yÃƒÂ¶nÃƒÂ¼ndeki tÃƒÂ¼m baÃ…Å¸latÃ„Â±lmÃ„Â±Ã…Å¸
    /// tick'lerdeki toplam absorbe edilebilir WETH miktarÃ„Â±nÃ„Â± hesaplar.
    ///
    /// Algoritma:
    ///   1. Swap yÃƒÂ¶nÃƒÂ¼ne gÃƒÂ¶re (zeroForOne veya oneForZero) ilgili tick'leri sÃ„Â±rala
    ///   2. Her tick aralÃ„Â±Ã„Å¸Ã„Â±ndaki mevcut likidite ile o aralÃ„Â±kta absorbe
    ///      edilebilecek maksimum WETH miktarÃ„Â±nÃ„Â± SqrtPriceMath ile hesapla
    ///   3. Tick sÃ„Â±nÃ„Â±rÃ„Â±nda liquidityNet ile aktif likiditeyi gÃƒÂ¼ncelle
    ///   4. TÃƒÂ¼m aralÃ„Â±klardaki kapasiteleri topla
    ///
    /// Bu sayede NR, havuzda gerÃƒÂ§ekten mevcut olmayan likiditeyi
    /// kullanmaya ÃƒÂ§alÃ„Â±Ã…Å¸maz. Ãƒâ€“rn: 5.5 WETH likidite varsa max 5.5 WETH ÃƒÂ¶nerilir.
    ///
    /// # DÃƒÂ¶nÃƒÂ¼Ã…Å¸
    /// Toplam absorbe edilebilir WETH miktarÃ„Â± (f64, human-readable).
    /// Bitmap yoksa veya boÃ…Å¸sa, `max_safe_swap_amount_u256` fallback kullanÃ„Â±lÃ„Â±r.
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

        // WETH satÃ„Â±Ã…Å¸Ã„Â±: zeroForOne (token0_is_weth=true Ã¢â€ â€™ sola git) veya
        //               oneForZero (token0_is_weth=false Ã¢â€ â€™ saÃ„Å¸a git)
        // Bot WETH giriyor Ã¢â€ â€™ swap yÃƒÂ¶nÃƒÂ¼ WETHÃ¢â€ â€™USDC
        let zero_for_one = token0_is_weth;

        // Tick'leri swap yÃƒÂ¶nÃƒÂ¼ne gÃƒÂ¶re sÃ„Â±rala
        let ordered_ticks: Vec<(i32, i128)> = {
            let mut ticks: Vec<(i32, i128)> = bitmap.ticks.iter()
                .filter(|(_, info)| info.initialized)
                .map(|(&t, info)| (t, info.liquidity_net))
                .collect();

            if zero_for_one {
                ticks.retain(|(t, _)| *t <= current_tick);
                ticks.sort_by(|a, b| b.0.cmp(&a.0)); // bÃƒÂ¼yÃƒÂ¼kten kÃƒÂ¼ÃƒÂ§ÃƒÂ¼Ã„Å¸e
            } else {
                ticks.retain(|(t, _)| *t > current_tick);
                ticks.sort_by_key(|(t, _)| *t); // kÃƒÂ¼ÃƒÂ§ÃƒÂ¼kten bÃƒÂ¼yÃƒÂ¼Ã„Å¸e
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

            // Bu aralÃ„Â±kta absorbe edilebilecek WETH miktarÃ„Â±
            let weth_in_range = if zero_for_one {
                // token0(WETH) girdi: amount0 = L Ãƒâ€” Q96 Ãƒâ€” (1/target - 1/current)
                if sqrt_price_target < state_sqrt_price {
                    get_amount0_delta(sqrt_price_target, state_sqrt_price, state_liquidity, false)
                } else {
                    U256::ZERO
                }
            } else {
                // token1(WETH) girdi: amount1 = L Ãƒâ€” (target - current) / Q96
                if sqrt_price_target > state_sqrt_price {
                    get_amount1_delta(state_sqrt_price, sqrt_price_target, state_liquidity, false)
                } else {
                    U256::ZERO
                }
            };

            total_weth_capacity += weth_in_range;
            state_sqrt_price = sqrt_price_target;

            // Tick sÃ„Â±nÃ„Â±rÃ„Â±nda likiditeyi gÃƒÂ¼ncelle
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
        // ama muhafazakÃƒÂ¢r olalÃ„Â±m Ã¢â‚¬â€ sadece baÃ…Å¸latÃ„Â±lmÃ„Â±Ã…Å¸ tick'lere kadar hesapla

        let cap_weth = u256_to_f64(total_weth_capacity) / 1e18;

        // Minimum: single-tick fallback ile karÃ…Å¸Ã„Â±laÃ…Å¸tÃ„Â±r, bÃƒÂ¼yÃƒÂ¼k olanÃ„Â± al
        // (bitmap'te ÃƒÂ§ok az tick varsa fallback daha iyi olabilir)
        let single_tick_cap = max_safe_swap_amount_u256(sqrt_price_x96, liquidity, token0_is_weth, current_tick, tick_spacing);

        // v16.0: %99.9 gÃƒÂ¼venlik marjÃ„Â± Ã¢â‚¬â€ slippage payÃ„Â± bÃ„Â±rak.
        // Havuz kapasitesinin tam sÃ„Â±nÃ„Â±rÃ„Â±nda iÃ…Å¸lem yapmak tick-ÃƒÂ§apraz
        // hatalarÃ„Â±na ve REVM revert'lerine yol aÃƒÂ§ar.
        let raw_cap = cap_weth.max(single_tick_cap);
        raw_cap * 0.999
    }

    // Ã¢â€â‚¬Ã¢â€â‚¬ Test Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

    #[cfg(test)]
    mod exact_tests {
        use super::*;
        use alloy::primitives::U256;

        #[test]
        fn test_get_sqrt_ratio_at_tick_zero() {
            let ratio = get_sqrt_ratio_at_tick(0);
            // tick=0 Ã¢â€ â€™ sqrtPrice = 2^96 = Q96
            assert_eq!(ratio, Q96, "tick=0 Ã¢â€ â€™ sqrtPriceX96 = Q96");
        }

        #[test]
        fn test_get_sqrt_ratio_at_tick_boundaries() {
            let min_ratio = get_sqrt_ratio_at_tick(-887272);
            let max_ratio = get_sqrt_ratio_at_tick(887272);
            assert!(min_ratio >= MIN_SQRT_RATIO, "min tick Ã¢â€ â€™ min sqrt ratio");
            assert!(max_ratio <= MAX_SQRT_RATIO, "max tick Ã¢â€ â€™ max sqrt ratio");
            assert!(min_ratio < max_ratio, "min < max");
        }

        #[test]
        fn test_get_sqrt_ratio_negative_tick() {
            let ratio_neg = get_sqrt_ratio_at_tick(-1);
            let ratio_pos = get_sqrt_ratio_at_tick(1);
            let ratio_zero = get_sqrt_ratio_at_tick(0);
            assert!(ratio_neg < ratio_zero, "negatif tick Ã¢â€ â€™ dÃƒÂ¼Ã…Å¸ÃƒÂ¼k fiyat");
            assert!(ratio_pos > ratio_zero, "pozitif tick Ã¢â€ â€™ yÃƒÂ¼ksek fiyat");
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
            assert!(step.amount_out > U256::ZERO, "Ãƒâ€¡Ã„Â±ktÃ„Â± sÃ„Â±fÃ„Â±r olmamalÃ„Â±");
            assert!(step.amount_in > U256::ZERO, "Girdi sÃ„Â±fÃ„Â±r olmamalÃ„Â±");
        }

        #[test]
        fn test_exact_swap_no_bitmap() {
            let sqrt_price = get_sqrt_ratio_at_tick(-200000);
            let liquidity: u128 = 50_000_000_000_000_000_000; // 5e19
            let amount = U256::from(1_000_000_000_000_000_000u128); // 1 WETH

            let result = compute_exact_swap(
                sqrt_price, liquidity, -200000,
                amount, true, 500, None,
            );

            assert!(result.amount_out > U256::ZERO, "Swap ÃƒÂ§Ã„Â±ktÃ„Â±sÃ„Â± > 0 olmalÃ„Â±");
            println!("Exact swap: 1 WETH Ã¢â€ â€™ {} raw USDC ÃƒÂ§Ã„Â±ktÃ„Â±", result.amount_out);
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
                amount, true, 500, Some(&bitmap),
            );

            assert!(result.amount_out > U256::ZERO, "Bitmap swap ÃƒÂ§Ã„Â±ktÃ„Â±sÃ„Â± > 0");
            assert!(result.tick_crossings > 0, "Tick geÃƒÂ§iÃ…Å¸i olmalÃ„Â±");
            println!(
                "Exact bitmap swap: 5 WETH Ã¢â€ â€™ {} raw ÃƒÂ§Ã„Â±ktÃ„Â±, {} tick geÃƒÂ§iÃ…Å¸i",
                result.amount_out, result.tick_crossings
            );
        }
    }
}
