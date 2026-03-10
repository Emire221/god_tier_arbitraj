// ============================================================================
//  TYPES Ã¢â‚¬â€ PaylaÃ…Å¸Ã„Â±lan Tipler, YapÃ„Â±landÃ„Â±rma ve Ã„Â°statistikler
//  Arbitraj Botu v9.0 Ã¢â‚¬â€ Base Network
//
//  v9.0 Yenilikler:
//  Ã¢Å“â€œ Executor/Admin rol ayrÃ„Â±mÃ„Â± (admin_address)
//  Ã¢Å“â€œ Deadline block desteÃ„Å¸i (deadline_blocks)
//  Ã¢Å“â€œ Dinamik bribe/priority fee modeli (bribe_pct)
//  Ã¢Å“â€œ Ã…Âifreli keystore desteÃ„Å¸i (keystore_path)
//  Ã¢Å“â€œ 134-byte calldata uyumu (deadlineBlock eklendi)
//
//  v7.0 (korunuyor):
//  Ã¢Å“â€œ NonceManager Ã¢â‚¬â€ AtomicU64 ile atomik nonce yÃƒÂ¶netimi
//  Ã¢Å“â€œ Token adresleri (weth_address, usdc_address) BotConfig'e eklendi
//  Ã¢Å“â€œ TickBitmap off-chain derinlik haritasÃ„Â± yapÃ„Â±larÃ„Â±
//  Ã¢Å“â€œ Multi-transport yapÃ„Â±landÃ„Â±rmasÃ„Â± (IPC > WSS > HTTP)
// ============================================================================

use alloy::primitives::{address, Address, U256};
use eyre::Result;
use std::collections::{HashMap, HashSet};
use std::time::Instant;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use parking_lot::RwLock;

// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
// Token Whitelist Ã¢â‚¬â€ GÃƒÂ¼venli Token Listesi (Base Network)
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
//
// v10.1: Sadece yÃƒÂ¼ksek likiditeli, kanÃ„Â±tlanmÃ„Â±Ã…Å¸ tokenlar beyaz listede.
// Egzotik veya yeni ÃƒÂ§Ã„Â±kan tokenlar ile iÃ…Å¸lem yapÃ„Â±lmasÃ„Â± engellenir.
// Bu, rÃƒÂ¼g-pull, dÃƒÂ¼Ã…Å¸ÃƒÂ¼k likidite kayasÃ„Â± ve token manipulasyonu risklerini
// ortadan kaldÃ„Â±rÃ„Â±r.
//
// Desteklenen tokenlar:
//   Ã¢â‚¬Â¢ WETH  Ã¢â‚¬â€ Wrapped Ether (Base canonical)
//   Ã¢â‚¬Â¢ USDC  Ã¢â‚¬â€ USD Coin (Circle, bridged)
//   Ã¢â‚¬Â¢ USDT  Ã¢â‚¬â€ Tether USD (bridged)
//   Ã¢â‚¬Â¢ DAI   Ã¢â‚¬â€ Dai Stablecoin (bridged)
//   Ã¢â‚¬Â¢ cbETH Ã¢â‚¬â€ Coinbase Wrapped Staked ETH
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

/// Base Network ÃƒÂ¼zerindeki gÃƒÂ¼venli token adresleri (donanÃ„Â±m kodlu whitelist)
pub fn token_whitelist() -> HashSet<Address> {
    HashSet::from([
        // WETH Ã¢â‚¬â€ Base canonical
        address!("4200000000000000000000000000000000000006"),
        // USDC Ã¢â‚¬â€ Circle (bridged)
        address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"),
        // USDbC Ã¢â‚¬â€ USD Base Coin (bridged via Base bridge)
        address!("d9aAEc86B65D86f6A7B5B1b0c42FFA531710b6CA"),
        // DAI Ã¢â‚¬â€ Dai Stablecoin (bridged)
        address!("50c5725949A6F0c72E6C4a641F24049A917DB0Cb"),
        // cbETH Ã¢â‚¬â€ Coinbase Wrapped Staked ETH
        address!("2Ae3F1Ec7F1F5012CFEab0185bfc7aa3cf0DEc22"),
        // cbBTC Ã¢â‚¬â€ Coinbase Wrapped BTC (8 decimals)
        address!("cbB7C0000aB88B473b1f5aFd9ef808440eed33Bf"),
    ])
}


/// uni_direction=0 Ã¢â€ â€™ zeroForOne=true  Ã¢â€ â€™ token0 input
/// uni_direction=1 Ã¢â€ â€™ zeroForOne=false Ã¢â€ â€™ token1 input
///
/// token0_is_weth=true:
///   - uni_dir=0 Ã¢â€ â€™ token0(WETH) input Ã¢â€ â€™ true
///   - uni_dir=1 Ã¢â€ â€™ token1(USDC) input Ã¢â€ â€™ false
///
/// token0_is_weth=false:
///   - uni_dir=0 Ã¢â€ â€™ token0(USDC) input Ã¢â€ â€™ false
///   - uni_dir=1 Ã¢â€ â€™ token1(WETH) input Ã¢â€ â€™ true
pub fn is_weth_input(uni_direction: u8, token0_is_weth: bool) -> bool {
    if uni_direction == 0 {
        // zeroForOne=true Ã¢â€ â€™ token0 is input
        token0_is_weth
    } else {
        // oneForZero=true â†' token1 is input
        !token0_is_weth
    }
}

/// WETH miktarÃ„Â±nÃ„Â± hedef token miktarÃ„Â±na ÃƒÂ§evir (human-readable Ã¢â€ â€™ wei).
///
/// - Hedef WETH ise: amount_weth * 10^18
/// - Hedef quote token ise: amount_weth * eth_price_quote * 10^quote_decimals
///
/// Bu fonksiyon calldata'ya yazÃ„Â±lacak amount deÃ„Å¸erini ÃƒÂ¼retir.
pub fn weth_amount_to_input_wei(
    optimal_amount_weth: f64,
    is_weth_input: bool,
    eth_price_quote: f64,
    quote_token_decimals: u8,
) -> U256 {
    if is_weth_input {
        // Input WETH Ã¢â€ â€™ 18 decimals
        U256::from(safe_f64_to_u128(optimal_amount_weth * 1e18))
    } else {
        // Input quote token Ã¢â€ â€™ quote_token_decimals
        // WETH cinsinden miktar Ãƒâ€” ETH/Quote fiyatÃ„Â± Ãƒâ€” 10^decimals
        let scale = 10f64.powi(quote_token_decimals as i32);
        let quote_amount = optimal_amount_weth * eth_price_quote * scale;
        U256::from(safe_f64_to_u128(quote_amount))
    }
}

/// f64 Ã¢â€ â€™ u128 gÃƒÂ¼venli dÃƒÂ¶nÃƒÂ¼Ã…Å¸ÃƒÂ¼m (saturating).
///
/// NaN, Infinity, negatif veya u128::MAX ÃƒÂ¼stÃƒÂ¼ deÃ„Å¸erler iÃƒÂ§in
/// Rust panic VERMEZ Ã¢â‚¬â€ yerine 0 veya u128::MAX dÃƒÂ¶ner.
/// MEV-kritik sistemlerde thread ÃƒÂ§ÃƒÂ¶kmesini ÃƒÂ¶nleyen savunma katmanÃ„Â±.
#[inline]
pub fn safe_f64_to_u128(val: f64) -> u128 {
    if val.is_nan() || val.is_infinite() || val < 0.0 {
        0
    } else if val >= u128::MAX as f64 {
        u128::MAX
    } else {
        // v22.0: Truncation Ã¢â€ â€™ rounding. Wei cinsinden 0.5+ kaybÃ„Â± ÃƒÂ¶nler.
        // Ãƒâ€“r: 1.9999 WETH Ã¢â€ â€™ 1 WEI (truncation) vs 2 WEI (rounding)
        val.round() as u128
    }
}

// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
// DEX TÃƒÂ¼rÃƒÂ¼
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DexType {
    UniswapV3,
    /// PancakeSwap V3 Ã¢â‚¬â€ slot0 feeProtocol alanÃ„Â± uint32 (Uniswap V3'te uint8)
    PancakeSwapV3,
    Aerodrome,
}

impl std::fmt::Display for DexType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DexType::UniswapV3 => write!(f, "Uniswap V3"),
            DexType::PancakeSwapV3 => write!(f, "PancakeSwap V3"),
            DexType::Aerodrome => write!(f, "Aerodrome"),
        }
    }
}

// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
// Transport Modu (L2 Sequencer Optimizasyonu)
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

/// BaÃ„Å¸lantÃ„Â± transport tipi Ã¢â‚¬â€ Base L2 iÃƒÂ§in IPC ÃƒÂ¶ncelikli
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportMode {
    /// IPC (Unix Domain Socket / Named Pipe) Ã¢â‚¬â€ En dÃƒÂ¼Ã…Å¸ÃƒÂ¼k gecikme (<0.1ms)
    Ipc,
    /// WebSocket Ã¢â‚¬â€ Orta gecikme (~1-5ms)
    Ws,
    /// HTTP Ã¢â‚¬â€ YÃƒÂ¼ksek gecikme (~5-50ms), fallback
    Http,
    /// Otomatik: IPC Ã¢â€ â€™ WSS Ã¢â€ â€™ HTTP sÃ„Â±rasÃ„Â±yla dener
    Auto,
}

impl std::fmt::Display for TransportMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportMode::Ipc => write!(f, "IPC (DÃƒÂ¼Ã…Å¸ÃƒÂ¼k Gecikme)"),
            TransportMode::Ws => write!(f, "WebSocket"),
            TransportMode::Http => write!(f, "HTTP"),
            TransportMode::Auto => write!(f, "Otomatik (IPCÃ¢â€ â€™WSSÃ¢â€ â€™HTTP)"),
        }
    }
}

// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
// TickBitmap YapÃ„Â±larÃ„Â± (Off-Chain Derinlik HaritasÃ„Â±)
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

/// Tek bir baÃ…Å¸latÃ„Â±lmÃ„Â±Ã…Å¸ tick'in bilgisi (Uniswap V3 ticks mapping)
///
/// Her tick sÃ„Â±nÃ„Â±rÃ„Â±nda likidite deÃ„Å¸iÃ…Å¸imi net olarak kaydedilir.
/// liquidityNet > 0 Ã¢â€ â€™ o tick'e girildiÃ„Å¸inde likidite ARTAR
/// liquidityNet < 0 Ã¢â€ â€™ o tick'e girildiÃ„Å¸inde likidite AZALIR
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct TickInfo {
    /// Toplam brÃƒÂ¼t likidite (pozisyon aÃƒÂ§ma/kapama iÃƒÂ§in)
    pub liquidity_gross: u128,
    /// Net likidite deÃ„Å¸iÃ…Å¸imi (tick geÃƒÂ§iÃ…Å¸inde uygulanÃ„Â±r)
    /// Pozitif: soldan saÃ„Å¸a geÃƒÂ§iÃ…Å¸te aktif likidite ARTAR
    /// Negatif: soldan saÃ„Å¸a geÃƒÂ§iÃ…Å¸te aktif likidite AZALIR
    pub liquidity_net: i128,
    /// Bu tick baÃ…Å¸latÃ„Â±lmÃ„Â±Ã…Å¸ mÃ„Â±? (bitmap'te 1 ise true)
    pub initialized: bool,
}

/// Off-chain TickBitmap derinlik haritasÃ„Â±
///
/// Zincirden ÃƒÂ§ekilen iki veri kaynaÃ„Å¸Ã„Â±nÃ„Â± birleÃ…Å¸tirir:
///   1. tickBitmap(int16 wordPos) Ã¢â€ â€™ uint256 : hangi tick'ler baÃ…Å¸latÃ„Â±lmÃ„Â±Ã…Å¸?
///   2. ticks(int24 tick) Ã¢â€ â€™ TickInfo : baÃ…Å¸latÃ„Â±lmÃ„Â±Ã…Å¸ tick'lerin detaylarÃ„Â±
///
/// Bu yapÃ„Â±, "50 ETH satarsam hangi 3 tick'i patlatÃ„Â±rÃ„Â±m?" sorusuna
/// mikrosaniye iÃƒÂ§inde cevap verir.
#[derive(Debug, Clone)]
pub struct TickBitmapData {
    /// Bitmap kelime haritasÃ„Â±: wordPos Ã¢â€ â€™ bitmap (256-bit)
    /// Her bit, tick_spacing'e gÃƒÂ¶re belirli bir tick'in baÃ…Å¸latÃ„Â±lmÃ„Â±Ã…Å¸ olup
    /// olmadÃ„Â±Ã„Å¸Ã„Â±nÃ„Â± gÃƒÂ¶sterir.
    pub words: HashMap<i16, U256>,

    /// BaÃ…Å¸latÃ„Â±lmÃ„Â±Ã…Å¸ tick'lerin detay bilgisi: tick Ã¢â€ â€™ TickInfo
    /// Sadece initialized=true olan tick'ler burada bulunur.
    pub ticks: HashMap<i32, TickInfo>,

    /// Bu verinin okunduÃ„Å¸u blok numarasÃ„Â±
    pub snapshot_block: u64,

    /// Senkronizasyon sÃƒÂ¼resi (mikrosaniye)
    pub sync_duration_us: u64,

    /// Taranan tick aralÃ„Â±Ã„Å¸Ã„Â± (current_tick Ã‚Â± range)
    pub scan_range: u32,
}

impl TickBitmapData {
    /// BoÃ…Å¸ bitmap oluÃ…Å¸tur
    pub fn empty() -> Self {
        Self {
            words: HashMap::new(),
            ticks: HashMap::new(),
            snapshot_block: 0,
            sync_duration_us: 0,
            scan_range: 0,
        }
    }

    /// Toplam baÃ…Å¸latÃ„Â±lmÃ„Â±Ã…Å¸ tick sayÃ„Â±sÃ„Â±
    pub fn initialized_tick_count(&self) -> usize {
        self.ticks.len()
    }
}

// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
// Havuz YapÃ„Â±landÃ„Â±rmasÃ„Â±
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

#[derive(Debug, Clone)]
pub struct PoolConfig {
    pub address: Address,
    pub name: String,
    pub fee_bps: u32,
    pub fee_fraction: f64,
    pub token0_decimals: u8,
    pub token1_decimals: u8,
    pub dex: DexType,
    /// token0 WETH mi? (Base: WETH < USDC adres sÃ„Â±rasÃ„Â±nda Ã¢â€ â€™ token0=WETH)
    pub token0_is_weth: bool,
    /// Tick aralÃ„Â±Ã„Å¸Ã„Â± (Uniswap V3 %0.05 = 10, Aerodrome deÃ„Å¸iÃ…Å¸ken)
    pub tick_spacing: i32,
    /// Quote token adresi (ÃƒÂ§ift bazlÃ„Â± Ã¢â‚¬â€ matched_pools.json'dan)
    pub quote_token_address: Address,
}

// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
// Havuz AnlÃ„Â±k Durumu (RAM'de tutulur)
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

#[derive(Debug, Clone)]
pub struct PoolState {
    /// sqrtPriceX96 (ham U256 deÃ„Å¸er)
    pub sqrt_price_x96: U256,
    /// sqrtPriceX96 float versiyonu (hÃ„Â±zlÃ„Â± hesap iÃƒÂ§in)
    pub sqrt_price_f64: f64,
    /// Mevcut tick
    pub tick: i32,
    /// AnlÃ„Â±k likidite (u128)
    pub liquidity: u128,
    /// Likidite float versiyonu (hÃ„Â±zlÃ„Â± hesap iÃƒÂ§in)
    pub liquidity_f64: f64,
    /// WETH fiyatÃ„Â± quote token cinsinden Ã¢â‚¬â€ ÃƒÂ¶r: 25.5 (cbBTC) veya 2500.0 (USDC)
    pub eth_price_usd: f64,
    /// Son gÃƒÂ¼ncellenen blok numarasÃ„Â±
    pub last_block: u64,
    /// Son gÃƒÂ¼ncelleme zamanÃ„Â± (yerel)
    pub last_update: Instant,
    /// Havuz baÃ…Å¸latÃ„Â±ldÃ„Â± mÃ„Â±?
    pub is_initialized: bool,
    /// Havuz bytecode'u (REVM iÃƒÂ§in ÃƒÂ¶nbellek)
    pub bytecode: Option<Vec<u8>>,
    /// Off-chain TickBitmap derinlik haritasÃ„Â±
    /// "50 ETH satarsam hangi tick'leri patlatÃ„Â±rÃ„Â±m?" sorusunu yanÃ„Â±tlar
    pub tick_bitmap: Option<TickBitmapData>,
    /// Zincirden okunan canlÃ„Â± fee (basis points, ÃƒÂ¶r: 500 = %0.05)
    /// None ise config'teki statik fee_bps kullanÃ„Â±lÃ„Â±r
    pub live_fee_bps: Option<u32>,
}

impl Default for PoolState {
    fn default() -> Self {
        Self {
            sqrt_price_x96: U256::ZERO,
            sqrt_price_f64: 0.0,
            tick: 0,
            liquidity: 0,
            liquidity_f64: 0.0,
            eth_price_usd: 0.0,
            last_block: 0,
            last_update: Instant::now(),
            is_initialized: false,
            bytecode: None,
            tick_bitmap: None,
            live_fee_bps: None,
        }
    }
}

impl PoolState {
    /// Havuz aktif mi? (veriler geÃƒÂ§erli mi?)
    pub fn is_active(&self) -> bool {
        self.is_initialized && self.eth_price_usd > 0.0 && self.liquidity > 0
    }

    /// Verinin yaÃ…Å¸Ã„Â± (milisaniye)
    pub fn staleness_ms(&self) -> u128 {
        self.last_update.elapsed().as_millis()
    }
}

/// Thread-safe havuz durumu
pub type SharedPoolState = Arc<RwLock<PoolState>>;

// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
// Dinamik Atomik Nonce YÃƒÂ¶neticisi
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

/// Lock-free, atomik nonce yÃƒÂ¶neticisi.
///
/// Problem: Her blokta `provider.get_transaction_count()` ÃƒÂ§aÃ„Å¸Ã„Â±rmak sÃ„Â±ralÃ„Â±
/// RPC gecikmesi yaratÃ„Â±r ve yarÃ„Â±Ã…Å¸ durumuna (race condition) aÃƒÂ§Ã„Â±ktÃ„Â±r.
///
/// Ãƒâ€¡ÃƒÂ¶zÃƒÂ¼m: Bot baÃ…Å¸langÃ„Â±cÃ„Â±nda nonce RPC'den bir kez okunur, sonra her TX
/// gÃƒÂ¶nderiminde atomik olarak artÃ„Â±rÃ„Â±lÃ„Â±r:
///
/// ```text
/// Bot baÃ…Å¸latÃ„Â±lÃ„Â±r Ã¢â€ â€™ RPC: eth_getTransactionCount Ã¢â€ â€™ nonce = 42
/// TX #1 gÃƒÂ¶nder Ã¢â€ â€™ nonce = 42, AtomicU64::fetch_add(1) Ã¢â€ â€™ nonce = 43
/// TX #2 gÃƒÂ¶nder Ã¢â€ â€™ nonce = 43, AtomicU64::fetch_add(1) Ã¢â€ â€™ nonce = 44
/// ```
///
/// SÃ„Â±fÃ„Â±r ek gecikme, sÃ„Â±fÃ„Â±r kilit ÃƒÂ§ekiÃ…Å¸mesi.
pub struct NonceManager {
    current_nonce: AtomicU64,
}

impl NonceManager {
    /// BaÃ…Å¸langÃ„Â±ÃƒÂ§ nonce deÃ„Å¸eriyle oluÃ…Å¸tur (RPC'den okunan deÃ„Å¸er)
    pub fn new(initial_nonce: u64) -> Self {
        Self {
            current_nonce: AtomicU64::new(initial_nonce),
        }
    }

    /// Mevcut nonce'u al ve atomik olarak 1 artÃ„Â±r.
    /// DÃƒÂ¶nen deÃ„Å¸er: TX'e yazÃ„Â±lacak nonce (artmadan ÃƒÂ¶nceki deÃ„Å¸er)
    pub fn get_and_increment(&self) -> u64 {
        self.current_nonce.fetch_add(1, Ordering::SeqCst)
    }

    /// Mevcut nonce'u oku (artÃ„Â±rmadan)
    pub fn current(&self) -> u64 {
        self.current_nonce.load(Ordering::SeqCst)
    }


    /// Nonce'u belirli bir deÃ„Å¸ere zorla ayarla (RPC senkronizasyonu iÃƒÂ§in)
    pub fn force_set(&self, nonce: u64) {
        self.current_nonce.store(nonce, Ordering::SeqCst);
    }
}

impl std::fmt::Debug for NonceManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NonceManager(nonce={})", self.current())
    }
}

// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
// Arbitraj FÃ„Â±rsatÃ„Â±
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

#[derive(Debug, Clone)]
pub struct ArbitrageOpportunity {
    /// Ucuz havuz indeksi (buradan al)
    pub buy_pool_idx: usize,
    /// PahalÃ„Â± havuz indeksi (buraya sat)
    pub sell_pool_idx: usize,
    /// Newton-Raphson ile hesaplanan optimal WETH miktarÃ„Â±
    pub optimal_amount_weth: f64,
    /// Beklenen net kÃƒÂ¢r (WETH cinsinden)
    pub expected_profit_weth: f64,
    /// AlÃ„Â±Ã…Å¸ fiyatÃ„Â± (ucuz havuz ETH/Quote)
    pub buy_price_quote: f64,
    /// SatÃ„Â±Ã…Å¸ fiyatÃ„Â± (pahalÃ„Â± havuz ETH/Quote)
    pub sell_price_quote: f64,
    /// Spread yÃƒÂ¼zdesi
    pub spread_pct: f64,
    /// Newton-Raphson yakÃ„Â±nsadÃ„Â± mÃ„Â±?
    pub nr_converged: bool,
    /// Newton-Raphson iterasyon sayÃ„Â±sÃ„Â±
    pub nr_iterations: u32,
}

// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
// REVM SimÃƒÂ¼lasyon Sonucu
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

#[derive(Debug, Clone)]
pub struct SimulationResult {
    /// SimÃƒÂ¼lasyon baÃ…Å¸arÃ„Â±lÃ„Â± mÃ„Â±?
    pub success: bool,
    /// KullanÃ„Â±lan gas
    pub gas_used: u64,
    /// Hata mesajÃ„Â± (varsa)
    pub error: Option<String>,
}

// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
// Bot YapÃ„Â±landÃ„Â±rmasÃ„Â± (.env tabanlÃ„Â±)
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

#[allow(dead_code)]
pub struct BotConfig {
    /// WebSocket RPC URL (blok baÃ…Å¸lÃ„Â±Ã„Å¸Ã„Â± aboneliÃ„Å¸i iÃƒÂ§in)
    pub rpc_wss_url: String,
    /// HTTP RPC URL (durum okuma iÃƒÂ§in Ã¢â‚¬â€ gelecekte kullanÃ„Â±labilir)
        pub rpc_http_url: String,
    /// IPC baÃ„Å¸lantÃ„Â± yolu (Unix socket / Windows named pipe)
        pub rpc_ipc_path: Option<String>,
    /// Transport modu (IPC > WSS > HTTP)
    pub transport_mode: TransportMode,
    /// Private key (kontrat tetikleme iÃƒÂ§in, opsiyonel)
    /// v9.0: KeyManager ÃƒÂ¼zerinden yÃƒÂ¶netilir, ama geriye uyumluluk iÃƒÂ§in saklanÃ„Â±r
    pub private_key: Option<String>,
    /// Arbitraj kontrat adresi (opsiyonel)
    pub contract_address: Option<Address>,
    /// WETH token adresi (Base: 0x4200000000000000000000000000000000000006)
    /// v12.0: Hardcoded Ã¢â‚¬â€ .env'den okunmaz, Base aÃ„Å¸Ã„Â±nda sabittir.
    pub weth_address: Address,
    /// Tahmini gas maliyeti fallback (WETH cinsinden)
    pub gas_cost_fallback_weth: f64,
    /// Flash loan ÃƒÂ¼creti (basis points)
    pub flash_loan_fee_bps: f64,
    /// Minimum net kÃƒÂ¢r eÃ…Å¸iÃ„Å¸i (WETH cinsinden)
    pub min_net_profit_weth: f64,
    /// Ã„Â°statistik gÃƒÂ¶sterme aralÃ„Â±Ã„Å¸Ã„Â± (blok sayÃ„Â±sÃ„Â±)
    pub stats_interval: u64,
    /// Maks yeniden baÃ„Å¸lanma denemesi (0 = sÃ„Â±nÃ„Â±rsÃ„Â±z)
    pub max_retries: u32,
    /// BaÃ…Å¸langÃ„Â±ÃƒÂ§ bekleme sÃƒÂ¼resi (saniye) Ã¢â‚¬â€ v10.1: agresif reconnect ile kullanÃ„Â±lmÃ„Â±yor
        pub initial_retry_delay_secs: u64,
    /// Maksimum bekleme sÃƒÂ¼resi (saniye) Ã¢â‚¬â€ v10.1: agresif reconnect ile kullanÃ„Â±lmÃ„Â±yor
        pub max_retry_delay_secs: u64,
    /// Veri tazelik eÃ…Å¸iÃ„Å¸i (milisaniye)
    pub max_staleness_ms: u128,
    /// Maksimum flash loan boyutu (WETH)
    pub max_trade_size_weth: f64,
    /// Base zincir ID
    pub chain_id: u64,
    /// TickBitmap tarama yarÃ„Â±ÃƒÂ§apÃ„Â± (mevcut tick Ã‚Â± range)
    /// VarsayÃ„Â±lan: 500 tick (Uniswap V3 %0.05 iÃƒÂ§in ~5% fiyat aralÃ„Â±Ã„Å¸Ã„Â±)
    pub tick_bitmap_range: u32,
    /// TickBitmap'in kaÃƒÂ§ blok eskiyene kadar geÃƒÂ§erli sayÃ„Â±lacaÃ„Å¸Ã„Â±
    pub tick_bitmap_max_age_blocks: u64,
    /// GÃƒÂ¶lge Modu (Shadow Mode): false ise fÃ„Â±rsatlar loglanÃ„Â±r, TX gÃƒÂ¶nderilmez
    /// .env'deki EXECUTION_ENABLED ile kontrol edilir
    pub execution_enabled_flag: bool,

    // Ã¢â€â‚¬Ã¢â€â‚¬ v9.0: Yeni GÃƒÂ¼venlik ve Performans AlanlarÃ„Â± Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

    /// Admin adresi Ã¢â‚¬â€ fon ÃƒÂ§ekme yetkisi (soÃ„Å¸uk cÃƒÂ¼zdan / multisig)
    /// v9.0 kontrat: admin rolÃƒÂ¼. BoÃ…Å¸sa executor adresi kullanÃ„Â±lÃ„Â±r.
        pub admin_address: Option<Address>,
    /// Deadline block offset Ã¢â‚¬â€ calldata'ya eklenir, kontrat kontrol eder
    /// Ãƒâ€“r: 2 Ã¢â€ â€™ mevcut blok + 2 = son geÃƒÂ§erli blok
    pub deadline_blocks: u32,
    /// Dinamik bribe yÃƒÂ¼zdesi Ã¢â‚¬â€ beklenen kÃƒÂ¢rÃ„Â±n bu oranÃ„Â± builder'a verilir
    /// Ãƒâ€“r: 0.25 = %25, coinbase.transfer veya yÃƒÂ¼ksek priority fee olarak
    pub bribe_pct: f64,
    /// Ã…Âifreli keystore dosya yolu (v9.0 key management)
        pub keystore_path: Option<String>,
    /// Key Manager modu aktif mi? (auto_load tarafÃ„Â±ndan ayarlanÃ„Â±r)
    pub key_manager_active: bool,
    /// v10.1: Circuit breaker eÃ…Å¸iÃ„Å¸i Ã¢â‚¬â€ kaÃƒÂ§ ardÃ„Â±Ã…Å¸Ã„Â±k baÃ…Å¸arÃ„Â±sÃ„Â±zlÃ„Â±kta bot kapanÃ„Â±r
    /// VarsayÃ„Â±lan: 3. .env'den CIRCUIT_BREAKER_THRESHOLD ile ayarlanabilir.
    pub circuit_breaker_threshold: u32,
    /// v15.0: Yedek RPC WebSocket URL (failover iÃƒÂ§in)
    /// Primary RPC'de hata veya yÃƒÂ¼ksek gecikme olursa backup'a geÃƒÂ§ilir.
    pub rpc_wss_url_backup: Option<String>,
    /// v15.0: Gecikme spike uyarÃ„Â± eÃ…Å¸iÃ„Å¸i (ms)
    /// Bu deÃ„Å¸erin ÃƒÂ¼zerinde gecikme loglanÃ„Â±r.
    pub latency_spike_threshold_ms: f64,
    /// v10.0: Private/Flashbots RPC URL (MEV korumasÃ„Â± iÃƒÂ§in)
    /// TanÃ„Â±mlÃ„Â±ysa eth_sendBundle kullanÃ„Â±lÃ„Â±r, deÃ„Å¸ilse public mempool
    pub private_rpc_url: Option<String>,
    /// v10.0: Ek WSS RPC URL'leri (Round-Robin havuz iÃƒÂ§in)
    /// Primary + backup dÃ„Â±Ã…Å¸Ã„Â±nda 3. endpoint
    pub rpc_wss_url_extra: Vec<String>,
    /// v21.0: Maksimum havuz komisyon tavanÃ„Â± (basis points)
    /// Bu deÃ„Å¸erin ÃƒÂ¼zerindeki fee'ye sahip havuzlar strateji deÃ„Å¸erlendirmesinde atlanÃ„Â±r.
    /// VarsayÃ„Â±lan: 30 bps (%0.30). .env'den MAX_POOL_FEE_BPS ile ayarlanabilir.
    /// v21.0: 100Ã¢â€ â€™30 dÃƒÂ¼Ã…Å¸ÃƒÂ¼rÃƒÂ¼ldÃƒÂ¼ Ã¢â‚¬â€ shadow mode analizleri yÃƒÂ¼ksek fee'li
    /// havuzlarÃ„Â±n kÃƒÂ¢rsÃ„Â±z olduÃ„Å¸unu gÃƒÂ¶sterdi. DÃƒÂ¼Ã…Å¸ÃƒÂ¼k fee (%0.01, %0.05) havuzlara odaklanÃ„Â±lÃ„Â±r.
    pub max_pool_fee_bps: u32,
}

impl BotConfig {
    /// .env dosyasÃ„Â±ndan yapÃ„Â±landÃ„Â±rmayÃ„Â± oku
    pub fn from_env() -> Result<Self> {
        let rpc_wss_url = std::env::var("RPC_WSS_URL")
            .map_err(|_| eyre::eyre!("RPC_WSS_URL .env dosyasÃ„Â±nda tanÃ„Â±mlanmalÃ„Â±dÃ„Â±r!"))?;

        if rpc_wss_url.is_empty() || rpc_wss_url.starts_with("wss://your-") {
            return Err(eyre::eyre!("RPC_WSS_URL geÃƒÂ§erli bir URL olmalÃ„Â±dÃ„Â±r!"));
        }

        // v15.0: Yedek RPC URL (opsiyonel)
        let rpc_wss_url_backup = std::env::var("RPC_WSS_URL_BACKUP")
            .ok()
            .filter(|u| !u.is_empty() && !u.starts_with("wss://your-"));

        let rpc_http_url = std::env::var("RPC_HTTP_URL")
            .map_err(|_| eyre::eyre!("RPC_HTTP_URL .env dosyasÃ„Â±nda tanÃ„Â±mlanmalÃ„Â±dÃ„Â±r!"))?;

        if rpc_http_url.is_empty() || rpc_http_url.starts_with("https://your-") {
            return Err(eyre::eyre!("RPC_HTTP_URL geÃƒÂ§erli bir URL olmalÃ„Â±dÃ„Â±r!"));
        }

        let private_key = std::env::var("PRIVATE_KEY")
            .ok()
            .filter(|pk| !pk.is_empty() && pk != "your-private-key-here");

        let contract_address = std::env::var("ARBITRAGE_CONTRACT_ADDRESS")
            .ok()
            .filter(|addr| !addr.is_empty() && addr != "0xYourContractAddress")
            .and_then(|addr| addr.parse::<Address>().ok());

        // Ã¢â€â‚¬Ã¢â€â‚¬ WETH Adresi (Base sabit) Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
        // v12.0: Legacy env var'lar (WETH_ADDRESS, QUOTE_TOKEN_*,
        // WETH_IS_TOKEN0, TOKEN0_DECIMALS, TOKEN1_DECIMALS) gÃƒÂ¶rmezden geliniyor.
        // Havuz bazlÃ„Â± token bilgileri matched_pools.json'dan geliyor.
        let weth_address: Address = address!("4200000000000000000000000000000000000006");

        let gas_cost_fallback_weth = Self::parse_env_f64("GAS_COST_FALLBACK_WETH", 0.00005);
        let flash_loan_fee_bps = Self::parse_env_f64("FLASH_LOAN_FEE_BPS", 5.0);
        // v22.0: Default 0.0001 Ã¢â€ â€™ 0.001 WETH (gas maliyetini karÃ…Å¸Ã„Â±layacak eÃ…Å¸ik)
        let min_net_profit_weth = Self::parse_env_f64("MIN_NET_PROFIT_WETH", 0.001);
        let max_trade_size_weth = Self::parse_env_f64("MAX_TRADE_SIZE_WETH", 50.0);

        let stats_interval = std::env::var("STATS_INTERVAL")
            .unwrap_or_else(|_| "10".into())
            .parse::<u64>()
            .unwrap_or(10);

        let max_retries = std::env::var("MAX_RETRIES")
            .unwrap_or_else(|_| "0".into())
            .parse::<u32>()
            .unwrap_or(0);

        let max_staleness_ms = std::env::var("MAX_STALENESS_MS")
            .unwrap_or_else(|_| "2000".into())
            .parse::<u128>()
            .unwrap_or(2000);

        let chain_id = std::env::var("CHAIN_ID")
            .unwrap_or_else(|_| "8453".into())
            .parse::<u64>()
            .unwrap_or(8453);

        // Ã¢â€â‚¬Ã¢â€â‚¬ IPC ve Transport AyarlarÃ„Â± Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
        let rpc_ipc_path = std::env::var("RPC_IPC_PATH")
            .ok()
            .filter(|p| !p.is_empty());

        let transport_mode = match std::env::var("TRANSPORT_MODE")
            .unwrap_or_else(|_| "auto".into())
            .to_lowercase()
            .as_str()
        {
            "ipc" => TransportMode::Ipc,
            "ws" | "wss" | "websocket" => TransportMode::Ws,
            "http" | "https" => TransportMode::Http,
            _ => TransportMode::Auto,
        };

        // Ã¢â€â‚¬Ã¢â€â‚¬ TickBitmap AyarlarÃ„Â± Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
        let tick_bitmap_range = std::env::var("TICK_BITMAP_RANGE")
            .unwrap_or_else(|_| "500".into())
            .parse::<u32>()
            .unwrap_or(500);

        let tick_bitmap_max_age_blocks = std::env::var("TICK_BITMAP_MAX_AGE_BLOCKS")
            .unwrap_or_else(|_| "5".into())
            .parse::<u64>()
            .unwrap_or(5);

        // Ã¢â€â‚¬Ã¢â€â‚¬ GÃƒÂ¶lge Modu (Shadow Mode) Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
        // EXECUTION_ENABLED=true Ã¢â€ â€™ gerÃƒÂ§ek TX gÃƒÂ¶nder
        // EXECUTION_ENABLED=false veya tanÃ„Â±msÃ„Â±z Ã¢â€ â€™ sadece logla
        let execution_enabled_flag = std::env::var("EXECUTION_ENABLED")
            .unwrap_or_else(|_| "false".into())
            .to_lowercase()
            .parse::<bool>()
            .unwrap_or(false);

        // Ã¢â€â‚¬Ã¢â€â‚¬ v9.0: Yeni GÃƒÂ¼venlik ve Performans AyarlarÃ„Â± Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

        // Admin adresi (fon ÃƒÂ§ekme yetkisi Ã¢â‚¬â€ kontrat v9.0)
        let admin_address = std::env::var("ADMIN_ADDRESS")
            .ok()
            .filter(|addr| !addr.is_empty())
            .and_then(|addr| addr.parse::<Address>().ok());

        // Deadline block offset (varsayÃ„Â±lan: 2 blok)
        let deadline_blocks = std::env::var("DEADLINE_BLOCKS")
            .unwrap_or_else(|_| "2".into())
            .parse::<u32>()
            .unwrap_or(2);

        // Dinamik bribe yÃƒÂ¼zdesi (varsayÃ„Â±lan: %25)
        let bribe_pct = Self::parse_env_f64("BRIBE_PCT", 0.25);

        // v10.1: Circuit breaker eÃ…Å¸iÃ„Å¸i (varsayÃ„Â±lan: 3)
        let circuit_breaker_threshold = std::env::var("CIRCUIT_BREAKER_THRESHOLD")
            .unwrap_or_else(|_| "3".into())
            .parse::<u32>()
            .unwrap_or(3);

        // Ã…Âifreli keystore dosya yolu
        let keystore_path = std::env::var("KEYSTORE_PATH")
            .ok()
            .filter(|p| !p.is_empty());

        Ok(Self {
            rpc_wss_url,
            rpc_http_url,
            rpc_ipc_path,
            transport_mode,
            private_key,
            contract_address,
            weth_address,
            gas_cost_fallback_weth,
            flash_loan_fee_bps,
            min_net_profit_weth,
            stats_interval,
            max_retries,
            initial_retry_delay_secs: 2,
            max_retry_delay_secs: 60,
            max_staleness_ms,
            max_trade_size_weth,
            chain_id,
            tick_bitmap_range,
            tick_bitmap_max_age_blocks,
            execution_enabled_flag,
            admin_address,
            deadline_blocks,
            bribe_pct,
            keystore_path,
            key_manager_active: false, // main.rs'de KeyManager baÃ…Å¸latÃ„Â±ldÃ„Â±ktan sonra gÃƒÂ¼ncellenir
            circuit_breaker_threshold,
            rpc_wss_url_backup,
            latency_spike_threshold_ms: Self::parse_env_f64("LATENCY_SPIKE_THRESHOLD_MS", 200.0),
            private_rpc_url: std::env::var("PRIVATE_RPC_URL")
                .ok()
                .filter(|u| !u.is_empty()),
            rpc_wss_url_extra: {
                let mut extras = Vec::new();
                // RPC_WSS_URL_2, RPC_WSS_URL_3 opsiyonel ek endpoint'ler
                for key in &["RPC_WSS_URL_2", "RPC_WSS_URL_3"] {
                    if let Ok(url) = std::env::var(key) {
                        if !url.is_empty() && !url.starts_with("wss://your-") {
                            extras.push(url);
                        }
                    }
                }
                extras
            },
            max_pool_fee_bps: std::env::var("MAX_POOL_FEE_BPS")
                .unwrap_or_else(|_| "10".into())
                .parse::<u32>()
                .unwrap_or(10),
        })
    }

    /// Kontrat tetikleme modu aktif mi?
    /// KoÃ…Å¸ullar:
    ///   1. EXECUTION_ENABLED=true (.env)
    ///   2. Private key mevcut (keystore VEYA env var)
    ///   3. ARBITRAGE_CONTRACT_ADDRESS tanÃ„Â±mlÃ„Â±
    pub fn execution_enabled(&self) -> bool {
        self.execution_enabled_flag
            && (self.private_key.is_some() || self.key_manager_active)
            && self.contract_address.is_some()
    }

    /// GÃƒÂ¶lge modu aktif mi? (Loglama yapÃ„Â±lÃ„Â±r ama TX gÃƒÂ¶nderilmez)
    pub fn shadow_mode(&self) -> bool {
        !self.execution_enabled_flag
    }

    /// .env'den f64 oku
    fn parse_env_f64(key: &str, default: f64) -> f64 {
        std::env::var(key)
            .unwrap_or_else(|_| default.to_string())
            .parse::<f64>()
            .unwrap_or(default)
    }
}

// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
// load_pool_configs_from_env() SÃ„Â°LÃ„Â°NDÃ„Â° Ã¢â‚¬â€ v11.0
// Havuz yapÃ„Â±landÃ„Â±rmasÃ„Â± artÃ„Â±k matched_pools.json'dan pool_discovery::build_runtime()
// ile yÃƒÂ¼klenir. Statik POOL_A/B_ADDRESS env var'larÃ„Â± kullanÃ„Â±lmaz.
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
// Arbitraj Ã„Â°statistikleri
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

pub struct ArbitrageStats {
    pub total_blocks_processed: u64,
    pub total_opportunities: u64,
    pub profitable_opportunities: u64,
    pub executed_trades: u64,
    pub failed_simulations: u64,
    pub max_spread_pct: f64,
    pub max_profit_weth: f64,
    pub total_potential_profit: f64,
    pub session_start: Instant,
    /// Transport tÃƒÂ¼rÃƒÂ¼ (aktif baÃ„Å¸lantÃ„Â±)
    pub active_transport: String,
    /// Ortalama blok iÃ…Å¸leme gecikmesi (ms)
    pub avg_block_latency_ms: f64,
    /// Minimum blok iÃ…Å¸leme gecikmesi (ms)
    pub min_block_latency_ms: f64,
    /// Toplam tick bitmap senkronizasyon sayÃ„Â±sÃ„Â±
    pub tick_bitmap_syncs: u64,
    /// v10.0: ArdÃ„Â±Ã…Å¸Ã„Â±k baÃ…Å¸arÃ„Â±sÃ„Â±zlÃ„Â±k sayacÃ„Â± (circuit breaker iÃƒÂ§in)
    /// 3 ardÃ„Â±Ã…Å¸Ã„Â±k simÃƒÂ¼lasyon/TX baÃ…Å¸arÃ„Â±sÃ„Â±zlÃ„Â±Ã„Å¸Ã„Â±nda bot geÃƒÂ§ici olarak durur
    pub consecutive_failures: u32,
    /// v15.0: Maksimum blok iÃ…Å¸leme gecikmesi (ms)
    pub max_block_latency_ms: f64,
    /// v15.0: Gecikme spike sayÃ„Â±sÃ„Â± (threshold ÃƒÂ¼zerinde)
    pub latency_spikes: u64,
    /// v23.0 (Y-1): GÃƒÂ¶lge modunda simÃƒÂ¼lasyon baÃ…Å¸arÃ„Â±lÃ„Â± fÃ„Â±rsat sayÃ„Â±sÃ„Â±
    pub shadow_sim_success: u64,
    /// v23.0 (Y-1): GÃƒÂ¶lge modunda simÃƒÂ¼lasyon baÃ…Å¸arÃ„Â±sÃ„Â±z fÃ„Â±rsat sayÃ„Â±sÃ„Â±
    pub shadow_sim_fail: u64,
    /// v23.0 (Y-1): GÃƒÂ¶lge modunda kÃƒÂ¼mÃƒÂ¼latif potansiyel kÃƒÂ¢r (WETH)
    pub shadow_cumulative_profit: f64,
}

impl ArbitrageStats {
    pub fn new() -> Self {
        Self {
            total_blocks_processed: 0,
            total_opportunities: 0,
            profitable_opportunities: 0,
            executed_trades: 0,
            failed_simulations: 0,
            max_spread_pct: 0.0,
            max_profit_weth: 0.0,
            total_potential_profit: 0.0,
            session_start: Instant::now(),
            active_transport: String::from("Bilinmiyor"),
            avg_block_latency_ms: 0.0,
            min_block_latency_ms: f64::MAX,
            tick_bitmap_syncs: 0,
            consecutive_failures: 0,
            max_block_latency_ms: 0.0,
            latency_spikes: 0,
            shadow_sim_success: 0,
            shadow_sim_fail: 0,
            shadow_cumulative_profit: 0.0,
        }
    }

    /// Blok gecikme istatistiÃ„Å¸ini gÃƒÂ¼ncelle
    pub fn update_latency(&mut self, latency_ms: f64) {
        if self.total_blocks_processed == 0 {
            self.avg_block_latency_ms = latency_ms;
        } else {
            // Kayan ortalama
            let n = self.total_blocks_processed as f64;
            self.avg_block_latency_ms = (self.avg_block_latency_ms * n + latency_ms) / (n + 1.0);
        }
        if latency_ms < self.min_block_latency_ms {
            self.min_block_latency_ms = latency_ms;
        }
        if latency_ms > self.max_block_latency_ms {
            self.max_block_latency_ms = latency_ms;
        }
    }

    pub fn uptime_str(&self) -> String {
        let secs = self.session_start.elapsed().as_secs();
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        let s = secs % 60;
        format!("{:02}:{:02}:{:02}", h, m, s)
    }
}
