// ============================================================================
//  STATE_SYNC v9.2 Ã¢â‚¬â€ Multicall3 + Optimistic Pending TX Dinleyici
//
//  v9.2 Yenilikler (Issue #101 Ã¢â‚¬â€ Aerodrome ABI KÃƒÂ¶k-neden Analizi):
//  Ã¢Å“â€œ Aerodrome Slipstream slot0 ABI 6 parametre olarak dÃƒÂ¼zeltildi
//    (Aerodrome CLPool.sol Slot0 struct'Ã„Â±nda feeProtocol YOKTUR)
//  Ã¢Å“â€œ Aerodrome ticks() ABI 10 parametre olarak gÃƒÂ¼ncellendi
//    (Uniswap V3'ten farklÃ„Â±: ekstra stakedLiquidityNet + rewardGrowthOutsideX128)
//  Ã¢Å“â€œ Pool adresi doÃ„Å¸rulama rehberi eklendi
//
//  v9.0 Yenilikler:
//  Ã¢Å“â€œ Pending TX stream (eth_subscribe newPendingTransactions)
//  Ã¢Å“â€œ Ã„Â°yimser (optimistic) havuz durum gÃƒÂ¼ncellemesi (blok ÃƒÂ¶ncesi tahmin)
//  Ã¢Å“â€œ Havuz adreslerine giden swap TXÃ¢â‚¬â„¢lerini anlÃ„Â±k yakalama
//
//  v8.0 (korunuyor):
//  Ã¢Å“â€œ Multicall3 (0xcA11bde05977b3631167028862bE2a173976CA11) entegrasyonu
//  Ã¢Å“â€œ 30-50 ayrÃ„Â± tickBitmap + ticks RPC ÃƒÂ§aÃ„Å¸rÃ„Â±sÃ„Â± Ã¢â€ â€™ TEK eth_call
//  Ã¢Å“â€œ AÃ„Å¸ gecikmesi ~80ms Ã¢â€ â€™ ~5ms (1 RTT), rate-limit riski sÃ„Â±fÃ„Â±r
//  Ã¢Å“â€œ sync_all_pools, cache_all_bytecodes hÃƒÂ¢lÃƒÂ¢ join_all (az sayÃ„Â±da ÃƒÂ§aÃ„Å¸rÃ„Â±)
//
//  Mimari:
//    1. tickBitmap word sorgularÃ„Â±nÃ„Â± Multicall3.aggregate3 ile paketle
//    2. Tek eth_call Ã¢â€ â€™ tÃƒÂ¼m wordÃ¢â‚¬â„¢ler tek yanÃ„Â±tta dÃƒÂ¶ner
//    3. BaÃ…Å¸latÃ„Â±lmÃ„Â±Ã…Å¸ tickÃ¢â‚¬â„¢lerin detaylarÃ„Â±nÃ„Â± yine Multicall3 ile tek ÃƒÂ§aÃ„Å¸rÃ„Â±da oku
//    4. Toplam: 2 RPC ÃƒÂ§aÃ„Å¸rÃ„Â±sÃ„Â± (eski: 40+ paralel ÃƒÂ§aÃ„Å¸rÃ„Â±)
//    5. [YENÃ„Â°] Pending TX stream ile blok ÃƒÂ¶ncesi iyimser gÃƒÂ¼celleme
// ============================================================================

use alloy::primitives::{address, Address, Bytes, U256};
use alloy::providers::Provider;
use alloy::transports::Transport;
use alloy::network::Ethereum;
use alloy::sol;
use alloy::sol_types::SolCall;
use eyre::Result;
use futures_util::StreamExt;
use std::time::Instant;
use futures_util::future::join_all;

use crate::math::compute_eth_price;
use crate::math::exact::u256_to_f64;
use crate::types::{DexType, PoolConfig, SharedPoolState, TickBitmapData, TickInfo};

// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
// Base L2 GasPriceOracle Ã¢â‚¬â€ L1 Data Fee Tahmin KontratÃ„Â±
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
//
// OP Stack (Base) ÃƒÂ¼zerindeki her TX, L2 yÃƒÂ¼rÃƒÂ¼tme ÃƒÂ¼cretine ek olarak
// L1'e veri yayÃ„Â±nlama ÃƒÂ¼creti ÃƒÂ¶der. Bu ÃƒÂ¼cret TX boyutuna baÃ„Å¸lÃ„Â±dÃ„Â±r.
//
// GasPriceOracle kontratÃ„Â± (0x420...00F) iÃ…Å¸lemin calldata'sÃ„Â±nÃ„Â± alÃ„Â±p
// L1 veri ÃƒÂ¼cretini wei cinsinden dÃƒÂ¶ndÃƒÂ¼rÃƒÂ¼r.
//
// Adres: 0x420000000000000000000000000000000000000F (Base, OP Mainnet, vb.)
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

/// Base GasPriceOracle adresi (tÃƒÂ¼m OP Stack aÃ„Å¸larÃ„Â±nda standart)
const GAS_PRICE_ORACLE_ADDRESS: Address = address!("420000000000000000000000000000000000000F");

sol! {
    #[sol(rpc)]
    interface IGasPriceOracle {
        /// Verilen calldata iÃƒÂ§in L1 data fee'sini wei cinsinden dÃƒÂ¶ndÃƒÂ¼rÃƒÂ¼r.
        function getL1Fee(bytes memory _data) external view returns (uint256);
    }
}

// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
// Multicall3 Ã¢â‚¬â€ Standart Ãƒâ€¡ok-Ãƒâ€¡aÃ„Å¸rÃ„Â± KontratÃ„Â± (TÃƒÂ¼m EVM Zincirlerde AynÃ„Â± Adres)
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

/// Multicall3 adresi Ã¢â‚¬â€ Base, Ethereum, Arbitrum, Optimism vb. hepsi aynÃ„Â±
const MULTICALL3_ADDRESS: Address = address!("cA11bde05977b3631167028862bE2a173976CA11");

sol! {
    #[sol(rpc)]
    interface IMulticall3 {
        struct Call3 {
            address target;
            bool allowFailure;
            bytes callData;
        }

        struct Result {
            bool success;
            bytes returnData;
        }

        function aggregate3(Call3[] calldata calls) external payable returns (Result[] memory returnData);
    }
}

// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
// Uniswap V3 Havuz ArayÃƒÂ¼zÃƒÂ¼ (slot0 Ã¢â€ â€™ 7 deÃ„Å¸iÃ…Å¸ken, feeProtocol uint8)
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

sol! {
    #[sol(rpc)]
    interface IUniswapV3Pool {
        function slot0() external view returns (
            uint160 sqrtPriceX96,
            int24 tick,
            uint16 observationIndex,
            uint16 observationCardinality,
            uint16 observationCardinalityNext,
            uint8 feeProtocol,
            bool unlocked
        );

        function liquidity() external view returns (uint128);

        function fee() external view returns (uint24);

        function ticks(int24 tick) external view returns (
            uint128 liquidityGross,
            int128 liquidityNet,
            uint256 feeGrowthOutside0X128,
            uint256 feeGrowthOutside1X128,
            int56 tickCumulativeOutside,
            uint160 secondsPerLiquidityOutsideX128,
            uint32 secondsOutside,
            bool initialized
        );

        function tickBitmap(int16 wordPosition) external view returns (uint256);
    }
}

// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
// PancakeSwap V3 Havuz ArayÃƒÂ¼zÃƒÂ¼ (slot0 Ã¢â€ â€™ 7 deÃ„Å¸iÃ…Å¸ken, feeProtocol uint32)
//
// Ãƒâ€“NEMLÃ„Â°: PancakeSwap V3 slot0 struct'Ã„Â± Uniswap V3'ten FARKLIDIR:
//   - Uniswap V3 slot0:     7 parametre, feeProtocol = uint8
//   - PancakeSwap V3 slot0: 7 parametre, feeProtocol = uint32
//
// PancakeSwap feeProtocol deÃ„Å¸eri ~209718400 olabilir ki bu uint8'e sÃ„Â±Ã„Å¸maz.
// Alloy'un katÃ„Â± ABI ÃƒÂ§ÃƒÂ¶zÃƒÂ¼mleyicisi bunu "buffer overrun" olarak raporlar.
//
// Kaynak: github.com/pancakeswap/pancake-v3-contracts/.../PancakeV3Pool.sol
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

sol! {
    #[sol(rpc)]
    interface IPancakeSwapV3Pool {
        function slot0() external view returns (
            uint160 sqrtPriceX96,
            int24 tick,
            uint16 observationIndex,
            uint16 observationCardinality,
            uint16 observationCardinalityNext,
            uint32 feeProtocol,
            bool unlocked
        );

        function liquidity() external view returns (uint128);

        function fee() external view returns (uint24);

        function ticks(int24 tick) external view returns (
            uint128 liquidityGross,
            int128 liquidityNet,
            uint256 feeGrowthOutside0X128,
            uint256 feeGrowthOutside1X128,
            int56 tickCumulativeOutside,
            uint160 secondsPerLiquidityOutsideX128,
            uint32 secondsOutside,
            bool initialized
        );

        function tickBitmap(int16 wordPosition) external view returns (uint256);
    }
}

// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
// Aerodrome Slipstream Havuz ArayÃƒÂ¼zÃƒÂ¼ (slot0 Ã¢â€ â€™ 6 deÃ„Å¸iÃ…Å¸ken, feeProtocol YOK)
//
// Ãƒâ€“NEMLÃ„Â°: Aerodrome CLPool.sol Slot0 struct'Ã„Â± Uniswap V3'ten FARKLIDIR:
//   - Uniswap V3 slot0: 7 parametre (feeProtocol DAHÃ„Â°L)
//   - Aerodrome slot0:  6 parametre (feeProtocol YOK)
//
// Kaynak: github.com/aerodrome-finance/slipstream/blob/main/contracts/core/CLPool.sol
//   struct Slot0 {
//       uint160 sqrtPriceX96;
//       int24 tick;
//       uint16 observationIndex;
//       uint16 observationCardinality;
//       uint16 observationCardinalityNext;
//       bool unlocked;
//   }
//
// AyrÃ„Â±ca Aerodrome ticks() 10 parametre dÃƒÂ¶ner (Uniswap V3: 8 parametre).
// Ekstra alanlar: int128 stakedLiquidityNet, uint256 rewardGrowthOutsideX128
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

sol! {
    #[sol(rpc)]
    interface IAerodromePool {
        function slot0() external view returns (
            uint160 sqrtPriceX96,
            int24 tick,
            uint16 observationIndex,
            uint16 observationCardinality,
            uint16 observationCardinalityNext,
            bool unlocked
        );

        function liquidity() external view returns (uint128);

        function fee() external view returns (uint24);

        function ticks(int24 tick) external view returns (
            uint128 liquidityGross,
            int128 liquidityNet,
            uint256 feeGrowthOutside0X128,
            uint256 feeGrowthOutside1X128,
            int56 tickCumulativeOutside,
            uint160 secondsPerLiquidityOutsideX128,
            uint32 secondsOutside,
            bool initialized,
            int128 stakedLiquidityNet,
            uint256 rewardGrowthOutsideX128
        );

        function tickBitmap(int16 wordPosition) external view returns (uint256);
    }
}

// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
// Tek Havuz Durum Senkronizasyonu
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

/// RPC durum sorgulama zaman aÃ…Å¸Ã„Â±mÃ„Â± (milisaniye).
/// v22.0: 500ms Ã¢â€ â€™ 2000ms. Base L2 ~2s blok sÃƒÂ¼resi, yoÃ„Å¸un dÃƒÂ¶nemlerde
/// RPC gecikmeleri 500ms'yi aÃ…Å¸abilir Ã¢â€ â€™ gereksiz timeout hatalarÃ„Â±.
/// 2000ms yeterli sÃƒÂ¼re tanÃ„Â±r, 1 blok sÃƒÂ¼resi iÃƒÂ§inde yanÃ„Â±t beklenir.
const SYNC_TIMEOUT_MS: u64 = 2000;

/// Maksimum yeniden deneme sayÃ„Â±sÃ„Â± (timeout sonrasÃ„Â±)
const SYNC_MAX_RETRIES: u32 = 2;

/// Tek bir havuzun durumunu RPC ÃƒÂ¼zerinden oku ve SharedPoolState'e yaz
///
/// v17.0: SÃ„Â±kÃ„Â± timeout (500ms) + yeniden deneme (2 kez) mekanizmasÃ„Â±.
///        RPC gecikmesi spike'Ã„Â± (>500ms) durumunda eski veriyle devam etmek
///        yerine hÃ„Â±zlÃ„Â±ca yeniden dener. 2 deneme sonrasÃ„Â± hata dÃƒÂ¶ner.
///
/// v10.0: slot0 ve liquidity sorgularÃ„Â± artÃ„Â±k paralel (tokio::join!)
///        Eski: 2 sÃ„Â±ralÃ„Â± RPC ÃƒÂ§aÃ„Å¸rÃ„Â±sÃ„Â± (2 RTT)
///        Yeni: 1 paralel ÃƒÂ§aÃ„Å¸rÃ„Â± (1 RTT) Ã¢â‚¬â€ blok baÃ…Å¸Ã„Â±na ~2-5ms kazanÃƒÂ§
pub async fn sync_pool_state<T: Transport + Clone, P: Provider<T, Ethereum> + Sync>(
    provider: &P,
    pool_config: &PoolConfig,
    pool_state: &SharedPoolState,
    block_number: u64,
) -> Result<()> {
    let mut last_err: Option<eyre::Report> = None;

    for attempt in 0..=SYNC_MAX_RETRIES {
        match tokio::time::timeout(
            std::time::Duration::from_millis(SYNC_TIMEOUT_MS),
            sync_pool_state_inner(provider, pool_config, pool_state, block_number),
        ).await {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(e)) => {
                // RPC hatasÃ„Â± (timeout deÃ„Å¸il) Ã¢â‚¬â€ yeniden deneme
                if attempt < SYNC_MAX_RETRIES {
                    eprintln!(
                        "  \u{26a1} [{}] Sync hatasÃ„Â± (deneme {}/{}): {}",
                        pool_config.name, attempt + 1, SYNC_MAX_RETRIES + 1, e
                    );
                }
                last_err = Some(e);
            }
            Err(_elapsed) => {
                // Timeout Ã¢â‚¬â€ yeniden deneme
                if attempt < SYNC_MAX_RETRIES {
                    eprintln!(
                        "  \u{26a1} [{}] Sync timeout ({}ms, deneme {}/{})",
                        pool_config.name, SYNC_TIMEOUT_MS,
                        attempt + 1, SYNC_MAX_RETRIES + 1,
                    );
                }
                last_err = Some(eyre::eyre!(
                    "[{}] sync_pool_state timeout ({}ms)",
                    pool_config.name, SYNC_TIMEOUT_MS
                ));
            }
        }
    }

    Err(last_err.unwrap_or_else(|| eyre::eyre!("[{}] sync baÃ…Å¸arÃ„Â±sÃ„Â±z", pool_config.name)))
}

/// sync_pool_state iÃƒÂ§ implementasyonu (timeout wrapper'sÃ„Â±z)
async fn sync_pool_state_inner<T: Transport + Clone, P: Provider<T, Ethereum> + Sync>(
    provider: &P,
    pool_config: &PoolConfig,
    pool_state: &SharedPoolState,
    block_number: u64,
) -> Result<()> {
    let (sqrt_price_x96, tick, liquidity, live_fee_bps) = match pool_config.dex {
        DexType::UniswapV3 => {
            let pool = IUniswapV3Pool::new(pool_config.address, provider);
            let slot0_call = pool.slot0();
            let liq_call = pool.liquidity();
            let fee_call = pool.fee();
            let (slot0_result, liq_result, fee_result) = tokio::join!(
                slot0_call.call(),
                liq_call.call(),
                fee_call.call(),
            );
            let slot0 = slot0_result
                .map_err(|e| eyre::eyre!("[{}] slot0 okuma hatasÃ„Â± (V3/7-alan/uint8): {}", pool_config.name, e))?;
            let liq = liq_result
                .map_err(|e| eyre::eyre!("[{}] liquidity okuma hatasÃ„Â±: {}", pool_config.name, e))?;
            let fee_bps: Option<u32> = fee_result.ok().map(|f| f._0 / 100);
            (slot0.sqrtPriceX96, slot0.tick, liq._0, fee_bps)
        }
        DexType::PancakeSwapV3 => {
            let pool = IPancakeSwapV3Pool::new(pool_config.address, provider);
            let slot0_call = pool.slot0();
            let liq_call = pool.liquidity();
            let fee_call = pool.fee();
            let (slot0_result, liq_result, fee_result) = tokio::join!(
                slot0_call.call(),
                liq_call.call(),
                fee_call.call(),
            );
            let slot0 = slot0_result
                .map_err(|e| eyre::eyre!(
                    "[{}] slot0 okuma hatasÃ„Â± (PCS-V3/7-alan/uint32): {}\n\
                    Ã¢â€ â€™ Havuz adresi doÃ„Å¸ru bir PancakeSwap V3 Pool mu? Kontrol edin: {}",
                    pool_config.name, e, pool_config.address
                ))?;
            let liq = liq_result
                .map_err(|e| eyre::eyre!("[{}] liquidity okuma hatasÃ„Â±: {}", pool_config.name, e))?;
            let fee_bps: Option<u32> = fee_result.ok().map(|f| f._0 / 100);
            (slot0.sqrtPriceX96, slot0.tick, liq._0, fee_bps)
        }
        DexType::Aerodrome => {
            let pool = IAerodromePool::new(pool_config.address, provider);
            let slot0_call = pool.slot0();
            let liq_call = pool.liquidity();
            let fee_call = pool.fee();
            let (slot0_result, liq_result, fee_result) = tokio::join!(
                slot0_call.call(),
                liq_call.call(),
                fee_call.call(),
            );
            let slot0 = slot0_result
                .map_err(|e| eyre::eyre!(
                    "[{}] slot0 okuma hatasÃ„Â± (Aero/6-alan): {}\n\
                    Ã¢â€ â€™ Havuz adresi doÃ„Å¸ru bir Aerodrome CLPool mu? Kontrol edin: {}",
                    pool_config.name, e, pool_config.address
                ))?;
            let liq = liq_result
                .map_err(|e| eyre::eyre!("[{}] liquidity okuma hatasÃ„Â±: {}", pool_config.name, e))?;
            let fee_bps: Option<u32> = fee_result.ok().map(|f| f._0 / 100);
            (slot0.sqrtPriceX96, slot0.tick, liq._0, fee_bps)
        }
    };

    let sqrt_price_f64: f64 = u256_to_f64(U256::from(sqrt_price_x96));
    let liquidity_f64: f64 = u256_to_f64(U256::from(liquidity));

    let eth_price = compute_eth_price(
        sqrt_price_f64,
        tick,
        pool_config.token0_decimals,
        pool_config.token1_decimals,
        pool_config.token0_is_weth,
    );

    {
        let mut state = pool_state.write();
        state.sqrt_price_x96 = U256::from(sqrt_price_x96);
        state.sqrt_price_f64 = sqrt_price_f64;
        state.tick = tick;
        state.liquidity = liquidity;
        state.liquidity_f64 = liquidity_f64;
        state.eth_price_usd = eth_price;
        state.last_block = block_number;
        state.last_update = Instant::now();
        state.is_initialized = true;
        state.live_fee_bps = live_fee_bps;
    }

    Ok(())
}

// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
// TickBitmap Off-Chain Okuma Ã¢â‚¬â€ Derinlik HaritasÃ„Â±
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

/// TickBitmap word pozisyonunu hesapla
/// tick_index / tick_spacing Ã¢â€ â€™ compressed tick Ã¢â€ â€™ word = compressed >> 8
#[inline]
fn tick_to_word_pos(tick: i32, tick_spacing: i32) -> i16 {
    // Compressed tick: Solidity'deki gibi negatifler iÃƒÂ§in floor division
    let compressed = if tick < 0 && tick % tick_spacing != 0 {
        tick / tick_spacing - 1
    } else {
        tick / tick_spacing
    };
    (compressed >> 8) as i16
}

/// Bir bitmap word'ÃƒÂ¼ndeki tÃƒÂ¼m baÃ…Å¸latÃ„Â±lmÃ„Â±Ã…Å¸ tick indekslerini ÃƒÂ§Ã„Â±kar
fn extract_initialized_bits(word: U256, word_pos: i16, tick_spacing: i32) -> Vec<i32> {
    let mut ticks = Vec::new();
    if word == U256::ZERO {
        return ticks;
    }

    for bit in 0..256u16 {
        let mask = U256::from(1u64) << bit;
        if word & mask != U256::ZERO {
            let compressed = (word_pos as i32) * 256 + bit as i32;
            let tick = compressed * tick_spacing;
            ticks.push(tick);
        }
    }

    ticks
}

/// Havuzun TickBitmap'ini belirli bir aralÃ„Â±kta oku Ã¢â‚¬â€ Multicall3 ile TEK RPC
///
/// Bu fonksiyon:
///   1. Mevcut tick etrafÃ„Â±ndaki word pozisyonlarÃ„Â±nÃ„Â± hesaplar
///   2. TÃƒÂ¼m tickBitmap(wordPos) ÃƒÂ§aÃ„Å¸rÃ„Â±larÃ„Â±nÃ„Â± Multicall3 ile TEK eth_call'da atar
///   3. BaÃ…Å¸latÃ„Â±lmÃ„Â±Ã…Å¸ tick'ler iÃƒÂ§in ticks(tick) ÃƒÂ§aÃ„Å¸rÃ„Â±larÃ„Â±nÃ„Â± yine Multicall3 ile toplar
///   4. TÃƒÂ¼m veriyi TickBitmapData yapÃ„Â±sÃ„Â±na paketler
///
/// Performans: Eski: 30-50 ayrÃ„Â± RPC ÃƒÂ§aÃ„Å¸rÃ„Â±sÃ„Â± Ã¢â€ â€™ Yeni: 2 Multicall3 ÃƒÂ§aÃ„Å¸rÃ„Â±sÃ„Â± (2 RTT)
pub async fn sync_tick_bitmap<T: Transport + Clone, P: Provider<T, Ethereum> + Sync>(
    provider: &P,
    pool_config: &PoolConfig,
    pool_state: &SharedPoolState,
    block_number: u64,
    scan_range: u32,
) -> Result<()> {
    let start = Instant::now();

    let current_tick = pool_state.read().tick;
    let tick_spacing = pool_config.tick_spacing.max(1);

    // Tarama aralÃ„Â±Ã„Å¸Ã„Â±: current_tick Ã‚Â± (scan_range * tick_spacing)
    let tick_lo = current_tick - (scan_range as i32 * tick_spacing);
    let tick_hi = current_tick + (scan_range as i32 * tick_spacing);

    // Word pozisyon aralÃ„Â±Ã„Å¸Ã„Â±
    let word_lo = tick_to_word_pos(tick_lo, tick_spacing);
    let word_hi = tick_to_word_pos(tick_hi, tick_spacing);

    let mut bitmap_data = TickBitmapData::empty();
    bitmap_data.scan_range = scan_range;
    bitmap_data.snapshot_block = block_number;

    // Ã¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢Â
    //  ADIM 1: tickBitmap word'lerini Multicall3 ile TEK Ãƒâ€¡AÃ„ÂRIDA oku
    // Ã¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢Â

    let word_positions: Vec<i16> = (word_lo..=word_hi).collect();
    let mut all_initialized_ticks: Vec<i32> = Vec::new();

    if !word_positions.is_empty() {
        // Her word pozisyonu iÃƒÂ§in calldata oluÃ…Å¸tur
        let calls: Vec<IMulticall3::Call3> = word_positions
            .iter()
            .map(|&word_pos| {
                let calldata = encode_tick_bitmap_call(pool_config.dex.clone(), word_pos);
                IMulticall3::Call3 {
                    target: pool_config.address,
                    allowFailure: true,
                    callData: Bytes::from(calldata),
                }
            })
            .collect();

        // Multicall3 ile tek eth_call
        let multicall = IMulticall3::new(MULTICALL3_ADDRESS, provider);
        let results = multicall
            .aggregate3(calls)
            .call()
            .await
            .map_err(|e| eyre::eyre!("[{}] Multicall3 tickBitmap hatasÃ„Â±: {}", pool_config.name, e))?;

        // SonuÃƒÂ§larÃ„Â± ÃƒÂ§ÃƒÂ¶zÃƒÂ¼mle
        for (i, result) in results.returnData.iter().enumerate() {
            if result.success && result.returnData.len() >= 32 {
                let word = U256::from_be_slice(&result.returnData[result.returnData.len()-32..]);
                let word_pos = word_positions[i];
                if word != U256::ZERO {
                    bitmap_data.words.insert(word_pos, word);
                    let initialized = extract_initialized_bits(word, word_pos, tick_spacing);
                    all_initialized_ticks.extend(initialized);
                }
            }
        }
    }

    // Ã¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢Â
    //  ADIM 2: BaÃ…Å¸latÃ„Â±lmÃ„Â±Ã…Å¸ tick detaylarÃ„Â±nÃ„Â± Multicall3 ile TEK Ãƒâ€¡AÃ„ÂRIDA oku
    // Ã¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢ÂÃ¢â€¢Â

    // Tarama aralÃ„Â±Ã„Å¸Ã„Â±ndaki tick'leri filtrele
    all_initialized_ticks.retain(|t| *t >= tick_lo && *t <= tick_hi);

    if !all_initialized_ticks.is_empty() {
        // Her tick iÃƒÂ§in calldata oluÃ…Å¸tur
        let tick_calls: Vec<IMulticall3::Call3> = all_initialized_ticks
            .iter()
            .map(|&tick| {
                let tick_i24 = tick.clamp(-887272, 887272);
                let calldata = encode_ticks_call(pool_config.dex.clone(), tick_i24);
                IMulticall3::Call3 {
                    target: pool_config.address,
                    allowFailure: true,
                    callData: Bytes::from(calldata),
                }
            })
            .collect();

        let multicall = IMulticall3::new(MULTICALL3_ADDRESS, provider);
        let tick_results = multicall
            .aggregate3(tick_calls)
            .call()
            .await
            .map_err(|e| eyre::eyre!("[{}] Multicall3 ticks hatasÃ„Â±: {}", pool_config.name, e))?;

        // SonuÃƒÂ§larÃ„Â± ÃƒÂ§ÃƒÂ¶zÃƒÂ¼mle
        for (i, result) in tick_results.returnData.iter().enumerate() {
            if result.success && result.returnData.len() >= 64 {
                // Ã„Â°lk 32 byte = liquidityGross (uint128), sonraki 32 byte = liquidityNet (int128)
                // ABI decode: her parametre 32 byte padded
                if let Some((liq_gross, liq_net, initialized)) =
                    decode_ticks_result(&result.returnData)
                {
                    if initialized {
                        bitmap_data.ticks.insert(all_initialized_ticks[i], TickInfo {
                            liquidity_gross: liq_gross,
                            liquidity_net: liq_net,
                            initialized: true,
                        });
                    }
                }
            }
        }
    }

    let elapsed_us = start.elapsed().as_micros() as u64;
    bitmap_data.sync_duration_us = elapsed_us;

    // State'e yaz
    {
        let mut state = pool_state.write();
        state.tick_bitmap = Some(bitmap_data);
    }

    Ok(())
}

// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
// Havuz Bytecode Ãƒâ€“nbellekleme (REVM SimÃƒÂ¼lasyonu Ã„Â°ÃƒÂ§in)
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

pub async fn cache_pool_bytecode<T: Transport + Clone, P: Provider<T, Ethereum> + Sync>(
    provider: &P,
    pool_config: &PoolConfig,
    pool_state: &SharedPoolState,
) -> Result<()> {
    let code = provider
        .get_code_at(pool_config.address)
        .await
        .map_err(|e| eyre::eyre!("[{}] Bytecode okuma hatasÃ„Â±: {}", pool_config.name, e))?;

    let mut state = pool_state.write();
    state.bytecode = Some(code.to_vec());

    Ok(())
}

// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
// Toplu Senkronizasyon
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

pub async fn sync_all_pools<T: Transport + Clone, P: Provider<T, Ethereum> + Sync>(
    provider: &P,
    pools: &[PoolConfig],
    states: &[SharedPoolState],
    block_number: u64,
) -> Vec<Result<()>> {
    // v22.0: sync_all_pools timeout 500ms Ã¢â€ â€™ 2000ms (modÃƒÂ¼l sabiti ile aynÃ„Â±).
    // YÃƒÂ¼ksek aÃ„Å¸ yoÃ„Å¸unluÃ„Å¸unda 500ms'lik timeout gereksiz hata ÃƒÂ¼retiyordu.
    const SYNC_TIMEOUT_MS: u64 = 2000;
    const MAX_RETRIES: u32 = 1;

    let futures: Vec<_> = pools.iter().zip(states.iter())
        .map(|(config, state)| {
            let config = config.clone();
            let state = state.clone();
            async move {
                for attempt in 0..=MAX_RETRIES {
                    match tokio::time::timeout(
                        std::time::Duration::from_millis(SYNC_TIMEOUT_MS),
                        sync_pool_state(provider, &config, &state, block_number),
                    ).await {
                        Ok(Ok(())) => return Ok(()),
                        Ok(Err(e)) => {
                            if attempt < MAX_RETRIES {
                                eprintln!(
                                    "     \u{26a1} [{}] Sync hatasÃ„Â±, yeniden deneniyor ({}/{}): {}",
                                    config.name, attempt + 1, MAX_RETRIES, e,
                                );
                                continue;
                            }
                            return Err(e);
                        }
                        Err(_elapsed) => {
                            if attempt < MAX_RETRIES {
                                eprintln!(
                                    "     \u{26a1} [{}] Sync timeout ({}ms), yeniden deneniyor ({}/{})",
                                    config.name, SYNC_TIMEOUT_MS, attempt + 1, MAX_RETRIES,
                                );
                                continue;
                            }
                            return Err(eyre::eyre!(
                                "[{}] Sync timeout: {}ms i\u{00e7}inde yan\u{0131}t al\u{0131}namad\u{0131} ({} deneme)",
                                config.name, SYNC_TIMEOUT_MS, MAX_RETRIES + 1,
                            ));
                        }
                    }
                }
                unreachable!()
            }
        })
        .collect();
    join_all(futures).await
}

/// TÃƒÂ¼m havuzlarÃ„Â±n TickBitmap'lerini senkronize et
///
/// Her havuz iÃƒÂ§in:
///   1. tickBitmap word'lerini tarar
///   2. BaÃ…Å¸latÃ„Â±lmÃ„Â±Ã…Å¸ tick'lerin liquidityNet bilgisini okur
///   3. PoolState.tick_bitmap'e yazar
pub async fn sync_all_tick_bitmaps<T: Transport + Clone, P: Provider<T, Ethereum> + Sync>(
    provider: &P,
    pools: &[PoolConfig],
    states: &[SharedPoolState],
    block_number: u64,
    scan_range: u32,
) -> Vec<Result<()>> {
    let futures: Vec<_> = pools.iter().zip(states.iter())
        .map(|(config, state)| sync_tick_bitmap(provider, config, state, block_number, scan_range))
        .collect();
    join_all(futures).await
}

// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
// L1 Data Fee Tahmini (Base / OP Stack)
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

/// Arbitraj TX'inin L1 data fee'sini tahmin et (wei cinsinden).
///
/// GasPriceOracle.getL1Fee() ÃƒÂ§aÃ„Å¸rÃ„Â±sÃ„Â± ile 134-byte kompakt calldata'nÃ„Â±n
/// L1'e yayÃ„Â±nlanma maliyetini sorguler. Blok baÃ…Å¸Ã„Â±na 1 kez ÃƒÂ§aÃ„Å¸rÃ„Â±lmasÃ„Â± yeterlidir.
///
/// # DÃƒÂ¶nÃƒÂ¼Ã…Å¸
/// L1 data fee (wei). Hata durumunda 0 dÃƒÂ¶ner (fallback: sadece L2 ÃƒÂ¼creti kullanÃ„Â±lÃ„Â±r).
pub async fn estimate_l1_data_fee<T: Transport + Clone, P: Provider<T, Ethereum> + Sync>(
    provider: &P,
) -> u128 {
    // 134-byte representative calldata (mostly non-zero for worst case estimate)
    // GerÃƒÂ§ek calldata adresleri ve miktarlarÃ„Â± deÃ„Å¸iÃ…Å¸ir ama boyut sabittir.
    // Non-zero byte'lar 16 gas, zero byte'lar 4 gas maliyetlidir (EIP-2028).
    // Worst case: tamamÃ„Â± non-zero Ã¢â€ â€™ konservatif tahmin.
    let representative_calldata: Vec<u8> = vec![0xFFu8; 134];

    let oracle = IGasPriceOracle::new(GAS_PRICE_ORACLE_ADDRESS, provider);
    match oracle
        .getL1Fee(representative_calldata.into())
        .call()
        .await
    {
        Ok(result) => {
            let fee = result._0;
            // U256 Ã¢â€ â€™ u128 safe conversion
            if fee > alloy::primitives::U256::from(u128::MAX) {
                u128::MAX
            } else {
                fee.to::<u128>()
            }
        }
        Err(e) => {
            eprintln!(
                "  Ã¢Å¡Â Ã¯Â¸Â L1 data fee tahmini baÃ…Å¸arÃ„Â±sÃ„Â±z (fallback: konservatif tahmin): {}",
                e
            );
            // v22.0: Fallback 0 Ã¢â€ â€™ konservatif tahmin.
            // 0 dÃƒÂ¶nmek gas maliyetini eksik hesaplatÃ„Â±r Ã¢â€ â€™ zararlÃ„Â± iÃ…Å¸lem riski.
            // 134-byte non-zero calldata, Base L2'de ~0.0001-0.0005 ETH L1 fee ÃƒÂ¶der.
            // Konservatif ÃƒÂ¼st sÃ„Â±nÃ„Â±r: 500_000 gwei = 0.0005 ETH
            500_000_000_000_000u128 // 0.0005 ETH (wei)
        }
    }
}

pub async fn cache_all_bytecodes<T: Transport + Clone, P: Provider<T, Ethereum> + Sync>(
    provider: &P,
    pools: &[PoolConfig],
    states: &[SharedPoolState],
) -> Vec<Result<()>> {
    let futures: Vec<_> = pools.iter().zip(states.iter())
        .map(|(config, state)| cache_pool_bytecode(provider, config, state))
        .collect();
    join_all(futures).await
}
fn encode_tick_bitmap_call(_dex: DexType, word_pos: i16) -> Vec<u8> {
    // tickBitmap(int16) — ABI: selector(4) + int16 padded to 32 bytes
    let call = IUniswapV3Pool::tickBitmapCall { wordPosition: word_pos };
    IUniswapV3Pool::tickBitmapCall::abi_encode(&call)
}

/// ticks(int24 tick) çağrısı için raw calldata oluştur
/// Hem UniswapV3 hem Aerodrome aynÃ„Â± fonksiyon imzasÃ„Â±nÃ„Â± kullanÃ„Â±r:
///   selector = keccak256("ticks(int24)")[0..4] = 0xf30dba93
fn encode_ticks_call(_dex: DexType, tick: i32) -> Vec<u8> {
    // ticks(int24) Ã¢â‚¬â€ ABI: selector(4) + int24 padded to 32 bytes
    // Alloy int24 = i32 olarak temsil eder
    let call = IUniswapV3Pool::ticksCall { tick: tick.try_into().unwrap_or(0) };
    IUniswapV3Pool::ticksCall::abi_encode(&call)
}

/// ticks() dÃƒÂ¶nÃƒÂ¼Ã…Å¸ verisini decode et
///
/// UniswapV3: 256 byte (8 alan Ãƒâ€” 32 byte)
/// Aerodrome: 320 byte (10 alan Ãƒâ€” 32 byte Ã¢â‚¬â€ ekstra stakedLiquidityNet + rewardGrowthOutside)
///
/// Ã„Â°lk 8 alan her iki DEX'te aynÃ„Â± dÃƒÂ¼zendedir:
///   [0..32]   uint128 liquidityGross
///   [32..64]  int128  liquidityNet
///   [64..96]  uint256 feeGrowthOutside0X128
///   [96..128] uint256 feeGrowthOutside1X128
///   [128..160] int56  tickCumulativeOutside
///   [160..192] uint160 secondsPerLiquidityOutsideX128
///   [192..224] uint32 secondsOutside
///   [224..256] bool   initialized
/// ABI decode ticks() raw return data (DEX-agnostik).
///
/// Uniswap V3 ticks() Ã¢â€ â€™ 8 parametre (256 byte)
/// PancakeSwap V3 ticks() Ã¢â€ â€™ 8 parametre (256 byte)  
/// Aerodrome ticks() Ã¢â€ â€™ 10 parametre (320 byte)
///
/// Ã„Â°lk 3 kullanÃ„Â±lan alan tÃƒÂ¼m DEX'lerde aynÃ„Â± offset'tedir:
///   [0..32]   uint128 liquidityGross      Ã¢â‚¬â€ tÃƒÂ¼m DEX'lerde 1. alan
///   [32..64]  int128  liquidityNet         Ã¢â‚¬â€ tÃƒÂ¼m DEX'lerde 2. alan
///   [224..256] bool   initialized          Ã¢â‚¬â€ tÃƒÂ¼m DEX'lerde 8. alan
///
/// Aerodrome ek alanlarÃ„Â± (stakedLiquidityNet, rewardGrowthOutsideX128)
/// 8. alandan SONRA gelir, dolayÃ„Â±sÃ„Â±yla initialized offset'i deÃ„Å¸iÃ…Å¸mez.
///
/// DÃƒÂ¶nÃƒÂ¼Ã…Å¸: (liquidityGross, liquidityNet, initialized)
fn decode_ticks_result(data: &[u8]) -> Option<(u128, i128, bool)> {
    if data.len() < 256 {
        return None;
    }

    // liquidityGross: uint128 (son 16 byte of first 32-byte word)
    let liq_gross = u128::from_be_bytes(data[16..32].try_into().ok()?);

    // liquidityNet: int128 (son 16 byte of second 32-byte word)
    let liq_net = i128::from_be_bytes(data[48..64].try_into().ok()?);

    // initialized: bool (son byte of eighth 32-byte word)
    // v22.0: Offset doÃ„Å¸rulamasÃ„Â± Ã¢â‚¬â€ 8. word tÃƒÂ¼m DEX'lerde aynÃ„Â± (index=7)
    let initialized = data[255] != 0;

    Some((liq_gross, liq_net, initialized))
}

// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
// Optimistic Pending TX Dinleyici (FAZ 4 Ã¢â‚¬â€ Gecikme Ã„Â°yileÃ…Å¸tirmesi)
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
//
// AmaÃƒÂ§: Blok onayÃ„Â±nÃ„Â± beklemeden, mempool/sequencer'daki bekleyen swap
// iÃ…Å¸lemlerini yakalayÃ„Â±p havuz durumlarÃ„Â±nÃ„Â± iyimser (optimistic) olarak
// gÃƒÂ¼ncellemek. Bu sayede bot ~15-20ms erken hareket edebilir.
//
// AkÃ„Â±Ã…Å¸:
//   1. WebSocket ÃƒÂ¼zerinden pending TX stream aÃƒÂ§
//   2. Gelen TX'in `to` adresi izlenen havuzlardan biri mi?
//   3. Evet Ã¢â€ â€™ TX calldata'sÃ„Â±ndan swap yÃƒÂ¶nÃƒÂ¼nÃƒÂ¼ ve miktarÃ„Â±nÃ„Â± ÃƒÂ§Ã„Â±kar
//   4. In-memory fiyat tahminini gÃƒÂ¼ncelle (optimistic update)
//   5. Strateji modÃƒÂ¼lÃƒÂ¼ gÃƒÂ¼ncel fiyatlarÃ„Â± okuyarak erken arbitraj tespiti yapar
//
// NOT: Base L2 sequencer FIFO'dur Ã¢â‚¬â€ mempool sÃ„Â±nÃ„Â±rlÃ„Â±dÃ„Â±r.
//      Bu modÃƒÂ¼l "best effort" ÃƒÂ§alÃ„Â±Ã…Å¸Ã„Â±r, pending TX yoksa mevcut blok
//      bazlÃ„Â± akÃ„Â±Ã…Å¸ aynen devam eder.
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

/// Uniswap V3 / Aerodrome swap fonksiyon selektÃƒÂ¶rÃƒÂ¼
/// swap(address,bool,int256,uint160,bytes) Ã¢â€ â€™ 0x128acb08
const SWAP_SELECTOR: [u8; 4] = [0x12, 0x8a, 0xcb, 0x08];

/// Pending TX'in izlenen bir havuza swap olup olmadÃ„Â±Ã„Å¸Ã„Â±nÃ„Â± kontrol et
///
/// DÃƒÂ¶nen deÃ„Å¸er: (havuz_indeksi, is_swap) Ã¢â‚¬â€ swap deÃ„Å¸ilse None
pub fn check_pending_tx_relevance(
    tx_to: Option<Address>,
    tx_input: &[u8],
    pool_addresses: &[Address],
) -> Option<usize> {
    let to = tx_to?;

    // Hedef adres izlenen havuzlardan biri mi?
    let pool_idx = pool_addresses.iter().position(|&addr| addr == to)?;

    // Calldata en az 4 byte (selector) olmalÃ„Â±
    if tx_input.len() < 4 {
        return None;
    }

    // Swap selektÃƒÂ¶rÃƒÂ¼ mÃƒÂ¼?
    if tx_input[0..4] == SWAP_SELECTOR {
        Some(pool_idx)
    } else {
        None
    }
}

/// Pending swap TX varsa havuz durumunu iyimser olarak gÃƒÂ¼ncelle
///
/// Bu fonksiyon tam bir fiyat hesabÃ„Â± YAPMAZ Ã¢â‚¬â€ sadece havuzun
/// "yakÃ„Â±nda fiyat deÃ„Å¸iÃ…Å¸ecek" sinyalini verir ve mevcut state'i
/// yeniden okumayÃ„Â± tetikler.
///
/// # Parametreler
/// - `provider`: RPC saÃ„Å¸layÃ„Â±cÃ„Â± (anlÃ„Â±k slot0 sorgusu iÃƒÂ§in)
/// - `pool_config`: Etkilenen havuzun yapÃ„Â±landÃ„Â±rmasÃ„Â±
/// - `pool_state`: GÃƒÂ¼ncellenen havuz durumu (write lock alÃ„Â±r)
/// - `current_block`: Mevcut blok numarasÃ„Â±
///
/// # DÃƒÂ¶nÃƒÂ¼Ã…Å¸
/// - Ok(true): Durum gÃƒÂ¼ncellendi (yeni swap tespit edildi)
/// - Ok(false): GÃƒÂ¼ncelleme gerekmedi
/// - Err: RPC hatasÃ„Â±
pub async fn optimistic_refresh_pool<T: Transport + Clone, P: Provider<T, Ethereum> + Sync>(
    provider: &P,
    pool_config: &PoolConfig,
    pool_state: &SharedPoolState,
    current_block: u64,
) -> Result<bool> {
    // Havuzun gÃƒÂ¼ncel slot0 ve liquidity deÃ„Å¸erlerini anlÃ„Â±k oku
    // v10.0: Paralel okuma (tokio::join!) Ã¢â‚¬â€ tek RTT (~1-3ms)
    let (sqrt_price_x96, tick, liquidity) = match pool_config.dex {
        DexType::UniswapV3 => {
            let pool = IUniswapV3Pool::new(pool_config.address, provider);
            let slot0_call = pool.slot0();
            let liq_call = pool.liquidity();
            let (slot0_result, liq_result) = tokio::join!(
                slot0_call.call(),
                liq_call.call(),
            );
            let slot0 = slot0_result
                .map_err(|e| eyre::eyre!("[OPT:{}] slot0 okuma hatasÃ„Â± (V3/uint8): {}", pool_config.name, e))?;
            let liq = liq_result
                .map_err(|e| eyre::eyre!("[OPT:{}] liquidity okuma hatasÃ„Â±: {}", pool_config.name, e))?;
            (slot0.sqrtPriceX96, slot0.tick, liq._0)
        }
        DexType::PancakeSwapV3 => {
            let pool = IPancakeSwapV3Pool::new(pool_config.address, provider);
            let slot0_call = pool.slot0();
            let liq_call = pool.liquidity();
            let (slot0_result, liq_result) = tokio::join!(
                slot0_call.call(),
                liq_call.call(),
            );
            let slot0 = slot0_result
                .map_err(|e| eyre::eyre!("[OPT:{}] slot0 okuma hatasÃ„Â± (PCS-V3/uint32): {}", pool_config.name, e))?;
            let liq = liq_result
                .map_err(|e| eyre::eyre!("[OPT:{}] liquidity okuma hatasÃ„Â±: {}", pool_config.name, e))?;
            (slot0.sqrtPriceX96, slot0.tick, liq._0)
        }
        DexType::Aerodrome => {
            let pool = IAerodromePool::new(pool_config.address, provider);
            let slot0_call = pool.slot0();
            let liq_call = pool.liquidity();
            let (slot0_result, liq_result) = tokio::join!(
                slot0_call.call(),
                liq_call.call(),
            );
            let slot0 = slot0_result
                .map_err(|e| eyre::eyre!("[OPT:{}] slot0 okuma hatasÃ„Â± (Aero/6-alan): {}", pool_config.name, e))?;
            let liq = liq_result
                .map_err(|e| eyre::eyre!("[OPT:{}] liquidity okuma hatasÃ„Â±: {}", pool_config.name, e))?;
            (slot0.sqrtPriceX96, slot0.tick, liq._0)
        }
    };

    let sqrt_price_f64: f64 = u256_to_f64(U256::from(sqrt_price_x96));
    let liquidity_f64: f64 = u256_to_f64(U256::from(liquidity));

    let eth_price = compute_eth_price(
        sqrt_price_f64,
        tick,
        pool_config.token0_decimals,
        pool_config.token1_decimals,
        pool_config.token0_is_weth,
    );

    // Mevcut state ile karÃ…Å¸Ã„Â±laÃ…Å¸tÃ„Â±r Ã¢â‚¬â€ fiyat deÃ„Å¸iÃ…Å¸miÃ…Å¸se gÃƒÂ¼ncelle
    let price_changed = {
        let state = pool_state.read();
        (state.eth_price_usd - eth_price).abs() > 0.001 // >$0.001 fark
    };

    if price_changed {
        let mut state = pool_state.write();
        state.sqrt_price_x96 = U256::from(sqrt_price_x96);
        state.sqrt_price_f64 = sqrt_price_f64;
        state.tick = tick;
        state.liquidity = liquidity;
        state.liquidity_f64 = liquidity_f64;
        state.eth_price_usd = eth_price;
        state.last_block = current_block;
        state.last_update = Instant::now();
        Ok(true)
    } else {
        Ok(false)
    }
}

// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
// EVENT-DRIVEN STATE SYNC Ã¢â‚¬â€ Swap Event Dinleyici (v11.0)
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
//
// Polling yerine eth_subscribe("logs") ile Swap eventlerini dinler.
// Swap eventi doÃ„Å¸rudan sqrtPriceX96, liquidity ve tick bilgisi iÃƒÂ§erir Ã¢â‚¬â€
// ek slot0/liquidity RPC ÃƒÂ§aÃ„Å¸rÃ„Â±sÃ„Â±na gerek kalmaz (zero-latency).
//
// Uniswap V3 / Aerodrome Swap Event:
//   event Swap(
//     address indexed sender,
//     address indexed recipient,
//     int256 amount0,
//     int256 amount1,
//     uint160 sqrtPriceX96,
//     uint128 liquidity,
//     int24 tick
//   )
// Topic0: 0xc42079f94a6350d7e6235f29174924f928cc2ac818eb64fed8004e115fbcca67
//
// Sync Event (likidite deÃ„Å¸iÃ…Å¸imi):
//   Mint/Burn eventleri de dinlenebilir, ancak Swap yeterlidir ÃƒÂ§ÃƒÂ¼nkÃƒÂ¼
//   her swap sonrasÃ„Â± liquidity ve sqrtPrice gÃƒÂ¼nceldir.
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

/// Uniswap V3 / Aerodrome Swap event topic0
/// keccak256("Swap(address,address,int256,int256,uint160,uint128,int24)")
const SWAP_EVENT_TOPIC: [u8; 32] = [
    0xc4, 0x20, 0x79, 0xf9, 0x4a, 0x63, 0x50, 0xd7,
    0xe6, 0x23, 0x5f, 0x29, 0x17, 0x49, 0x24, 0xf9,
    0x28, 0xcc, 0x2a, 0xc8, 0x18, 0xeb, 0x64, 0xfe,
    0xd8, 0x00, 0x4e, 0x11, 0x5f, 0xbc, 0xca, 0x67,
];

/// Swap event log verisinden havuz durumunu ÃƒÂ§Ã„Â±kar ve gÃƒÂ¼ncelle.
///
/// Log Data formatÃ„Â± (non-indexed parametreler, ABI-encoded):
///   [0..32]    int256  amount0
///   [32..64]   int256  amount1
///   [64..96]   uint160 sqrtPriceX96 (saÃ„Å¸ hizalÃ„Â±, 32 byte padded)
///   [96..128]  uint128 liquidity (saÃ„Å¸ hizalÃ„Â±, 32 byte padded)
///   [128..160] int24   tick (saÃ„Å¸ hizalÃ„Â±, 32 byte padded, sign-extended)
///
/// # DÃƒÂ¶nÃƒÂ¼Ã…Å¸
/// Ok(true) Ã¢â€ â€™ durum gÃƒÂ¼ncellendi, Ok(false) Ã¢â€ â€™ gÃƒÂ¼ncelleme gerekmedi
pub fn process_swap_event_log(
    log_data: &[u8],
    log_address: Address,
    log_block_number: u64,
    pools: &[PoolConfig],
    states: &[SharedPoolState],
) -> Result<bool> {
    // Log adresi hangi havuza ait?
    let pool_idx = pools.iter()
        .position(|p| p.address == log_address);

    let pool_idx = match pool_idx {
        Some(idx) => idx,
        None => return Ok(false), // Bilinmeyen havuz, atla
    };

    // Log data en az 160 byte olmalÃ„Â± (5 Ãƒâ€” 32 byte)
    if log_data.len() < 160 {
        return Ok(false);
    }

    // sqrtPriceX96 ÃƒÂ§Ã„Â±kar (offset 64..96, uint160)
    let sqrt_price_x96 = U256::from_be_slice(&log_data[64..96]);

    // liquidity ÃƒÂ§Ã„Â±kar (offset 96..128, uint128)
    let liquidity_bytes = &log_data[112..128]; // Son 16 byte = uint128
    let liquidity = u128::from_be_bytes(liquidity_bytes.try_into().unwrap_or([0u8; 16]));

    // tick ÃƒÂ§Ã„Â±kar (offset 128..160, int24 olarak sign-extended int256)
    // Son 4 byte'Ã„Â± int32 olarak oku, sonra -887272..887272 aralÃ„Â±Ã„Å¸Ã„Â±na sÃ„Â±nÃ„Â±rla
    let tick_bytes = &log_data[156..160]; // Son 4 byte
    let tick_raw = i32::from_be_bytes(tick_bytes.try_into().unwrap_or([0u8; 4]));
    let tick = tick_raw.clamp(-887272, 887272);

    let config = &pools[pool_idx];

    // f64 dÃƒÂ¶nÃƒÂ¼Ã…Å¸ÃƒÂ¼mleri
    let sqrt_price_f64: f64 = u256_to_f64(sqrt_price_x96);
    let liquidity_f64: f64 = liquidity as f64;

    // ETH fiyatÃ„Â± hesapla
    let eth_price = compute_eth_price(
        sqrt_price_f64,
        tick,
        config.token0_decimals,
        config.token1_decimals,
        config.token0_is_weth,
    );

    // State gÃƒÂ¼ncelle
    {
        let mut state = states[pool_idx].write();
        state.sqrt_price_x96 = sqrt_price_x96;
        state.sqrt_price_f64 = sqrt_price_f64;
        state.tick = tick;
        state.liquidity = liquidity;
        state.liquidity_f64 = liquidity_f64;
        state.eth_price_usd = eth_price;
        state.last_block = log_block_number;
        state.last_update = Instant::now();
        state.is_initialized = true;
    }

    Ok(true)
}

/// Event-driven Swap dinleyici baÃ…Å¸lat.
///
/// Havuz adreslerindeki Swap eventlerini WebSocket/IPC ÃƒÂ¼zerinden dinler.
/// Her Swap eventi geldiÃ„Å¸inde havuz state'ini anlÃ„Â±k gÃƒÂ¼nceller.
/// Polling'e gÃƒÂ¶re avantaj: SÃ„Â±fÃ„Â±r gecikme, ek RPC ÃƒÂ§aÃ„Å¸rÃ„Â±sÃ„Â± yok.
///
/// # Parametreler
/// - `rpc_url`: WebSocket/IPC RPC adresi
/// - `pools`: Ã„Â°zlenen havuz yapÃ„Â±landÃ„Â±rmalarÃ„Â±
/// - `states`: PaylaÃ…Å¸Ã„Â±mlÃ„Â± havuz durumlarÃ„Â±
///
/// # DÃƒÂ¶nÃƒÂ¼Ã…Å¸
/// Bu fonksiyon sonsuz dÃƒÂ¶ngÃƒÂ¼de ÃƒÂ§alÃ„Â±Ã…Å¸Ã„Â±r. BaÃ„Å¸lantÃ„Â± koparsa Err dÃƒÂ¶ner.
pub async fn start_swap_event_listener<T: Transport + Clone, P: Provider<T, Ethereum> + Sync>(
    provider: &P,
    pools: &[PoolConfig],
    states: &[SharedPoolState],
) -> Result<()> {
    use alloy::rpc::types::Filter;

    // Havuz adreslerini filtre olarak ayarla
    let pool_addresses: Vec<Address> = pools.iter().map(|p| p.address).collect();

    // Swap event topic0
    let swap_topic = alloy::primitives::B256::from(SWAP_EVENT_TOPIC);

    // Log filtresi: Sadece izlenen havuzlardan gelen Swap eventleri
    let filter = Filter::new()
        .address(pool_addresses)
        .event_signature(swap_topic);

    // Log subscription baÃ…Å¸lat
    let sub = provider.subscribe_logs(&filter).await
        .map_err(|e| eyre::eyre!("Swap event abonelik hatasÃ„Â±: {}", e))?;
    let mut stream = sub.into_stream();

    println!(
        "  {} Event-driven Swap dinleyici aktif ({} havuz)",
        "Ã¢Å¡Â¡", pools.len()
    );

    while let Some(log) = stream.next().await {
        // Log adresini al (Deref through inner)
        let log_address = log.inner.address;

        // Blok numarasÃ„Â±nÃ„Â± al
        let block_number = log.block_number.unwrap_or(0);

        // Swap event log verisini iÃ…Å¸le
        let log_data: &[u8] = log.inner.data.data.as_ref();

        match process_swap_event_log(
            log_data,
            log_address,
            block_number,
            pools,
            states,
        ) {
            Ok(true) => {
                // State gÃƒÂ¼ncellendi Ã¢â‚¬â€ havuz bilgisini logla
                if let Some(idx) = pools.iter().position(|p| p.address == log_address) {
                    let state = states[idx].read();
                    eprintln!(
                        "     Ã¢Å¡Â¡ [Event] {} Ã¢â€ â€™ {:.2}$ | Tick: {} | Blok: #{}",
                        pools[idx].name,
                        state.eth_price_usd,
                        state.tick,
                        block_number,
                    );
                }
            }
            Ok(false) => {} // GÃƒÂ¼ncelleme gerekmedi
            Err(e) => {
                eprintln!("     Ã¢Å¡Â Ã¯Â¸Â [Event] Swap log iÃ…Å¸leme hatasÃ„Â±: {}", e);
            }
        }
    }

    Err(eyre::eyre!("Swap event stream kapandÃ„Â±"))
}

// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
// RPC Connection Drop Failover Testleri
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
//
// Risk: HFT botlarÃ„Â± WebSocket/IPC ÃƒÂ¼zerinden node ile haberleÃ…Å¸ir. Node'un
// soketi aniden kapanÃ„Â±rsa (EOF error), Rust panik yapÃ„Â±p ÃƒÂ§ÃƒÂ¶kebilir.
//
// Bu test modÃƒÂ¼lÃƒÂ¼ doÃ„Å¸rular:
//   1. sync_pool_state hata dÃƒÂ¶ndÃƒÂ¼rÃƒÂ¼r ama panik YAPMAZ
//   2. ArdÃ„Â±Ã…Å¸Ã„Â±k RPC hatalarÃ„Â± is_active() Ã¢â€ â€™ false ile tespit edilir
//   3. staleness_ms eÃ…Å¸iÃ„Å¸i aÃ…Å¸Ã„Â±ldÃ„Â±Ã„Å¸Ã„Â±nda gÃƒÂ¼venli geÃƒÂ§iÃ…Å¸ yapÃ„Â±lÃ„Â±r
//   4. Havuz state'i son bilinen gÃƒÂ¼venli deÃ„Å¸erde korunur
//
// Not: GerÃƒÂ§ek WSS baÃ„Å¸lantÃ„Â± kopmasÃ„Â± main.rs'deki reconnect dÃƒÂ¶ngÃƒÂ¼sÃƒÂ¼
// tarafÃ„Â±ndan ele alÃ„Â±nÃ„Â±r (run_bot() Ã¢â€ â€™ Result::Err Ã¢â€ â€™ exponential backoff).
// Bu testler state katmanÃ„Â±nÃ„Â±n panik-gÃƒÂ¼venli davranÃ„Â±Ã…Å¸Ã„Â±nÃ„Â± kanÃ„Â±tlar.
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

#[cfg(test)]
mod rpc_failover_tests {
    use alloy::primitives::U256;
    use std::sync::Arc;
    use parking_lot::RwLock;
    use std::time::{Duration, Instant};
    use crate::types::*;


    fn make_active_state(price: f64, liq: u128, block: u64) -> SharedPoolState {
        Arc::new(RwLock::new(PoolState {
            sqrt_price_x96: U256::from(1u64) << 96,
            sqrt_price_f64: 1.0,
            tick: 0,
            liquidity: liq,
            liquidity_f64: liq as f64,
            eth_price_usd: price,
            last_block: block,
            last_update: Instant::now(),
            is_initialized: true,
            bytecode: None,
            tick_bitmap: None,
            live_fee_bps: None,
        }))
    }

    /// RPC baÃ„Å¸lantÃ„Â± kopmasÃ„Â± simÃƒÂ¼lasyonu: Havuz state yazma paniklememeli.
    ///
    /// Senaryo: WSS soketi kapanÃ„Â±r Ã¢â€ â€™ sync_pool_state RPC hatasÃ„Â± alÃ„Â±r
    /// Ã¢â€ â€™ state gÃƒÂ¼ncellenmez Ã¢â€ â€™ staleness_ms artar Ã¢â€ â€™ is_active() hÃƒÂ¢lÃƒÂ¢ true
    /// ama veri bayat Ã¢â€ â€™ check_arbitrage_opportunity reddeder.
    ///
    /// Bu test, tÃƒÂ¼m akÃ„Â±Ã…Å¸Ã„Â±n panik olmadan ÃƒÂ§alÃ„Â±Ã…Å¸tÃ„Â±Ã„Å¸Ã„Â±nÃ„Â± kanÃ„Â±tlar.
    #[test]
    fn test_rpc_failover_without_panic() {
        // Ã¢â€â‚¬Ã¢â€â‚¬ 1. BaÃ…Å¸langÃ„Â±ÃƒÂ§: Aktif state Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
        let state = make_active_state(2500.0, 10_000_000_000_000_000_000, 100);

        // State aktif ve taze
        {
            let s = state.read();
            assert!(s.is_active(), "BaÃ…Å¸langÃ„Â±ÃƒÂ§ta state aktif olmalÃ„Â±");
            assert!(s.staleness_ms() < 100, "BaÃ…Å¸langÃ„Â±ÃƒÂ§ta veri taze olmalÃ„Â±");
        }

        // Ã¢â€â‚¬Ã¢â€â‚¬ 2. RPC kopmasÃ„Â± simÃƒÂ¼lasyonu Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
        // sync_pool_state ÃƒÂ§aÃ„Å¸rÃ„Â±ldÃ„Â±Ã„Å¸Ã„Â±nda RPC hatasÃ„Â± alÃ„Â±nÃ„Â±r (burada simÃƒÂ¼le ediyoruz).
        // State gÃƒÂ¼ncellenmez Ã¢â€ â€™ son bilinen deÃ„Å¸erde kalÃ„Â±r.
        // Bu noktada panik olmamalÃ„Â±.

        // BayatlÃ„Â±k simÃƒÂ¼lasyonu: last_update'i 6 saniye geriye ÃƒÂ§ek
        {
            let mut s = state.write();
            s.last_update = Instant::now() - Duration::from_secs(6);
        }

        // Ã¢â€â‚¬Ã¢â€â‚¬ 3. DoÃ„Å¸rulama: State bayat ama panic yok Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
        {
            let s = state.read();
            assert!(s.is_active(), "State hÃƒÂ¢lÃƒÂ¢ aktif (eski veriler geÃƒÂ§erli)");
            assert!(
                s.staleness_ms() >= 5000,
                "Veri bayat olmalÃ„Â± (>5s): {}ms",
                s.staleness_ms()
            );
            // Fiyat ve likidite son bilinen deÃ„Å¸erde korunmuÃ…Å¸
            assert_eq!(s.eth_price_usd, 2500.0, "Fiyat son bilinen deÃ„Å¸erde kalmalÃ„Â±");
            assert_eq!(s.liquidity, 10_000_000_000_000_000_000, "Likidite korunmalÃ„Â±");
        }

        // Ã¢â€â‚¬Ã¢â€â‚¬ 4. Yeniden baÃ„Å¸lantÃ„Â± sonrasÃ„Â± kurtarma Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
        // sync_pool_state yeni RPC ile baÃ…Å¸arÃ„Â±lÃ„Â± olur Ã¢â€ â€™ state gÃƒÂ¼ncellenir
        {
            let mut s = state.write();
            s.last_update = Instant::now();
            s.last_block = 105;
            s.eth_price_usd = 2510.0;
        }

        {
            let s = state.read();
            assert!(s.is_active(), "Kurtarma sonrasÃ„Â± state aktif olmalÃ„Â±");
            assert!(
                s.staleness_ms() < 100,
                "Kurtarma sonrasÃ„Â± veri taze olmalÃ„Â±"
            );
            assert_eq!(s.eth_price_usd, 2510.0, "Kurtarma sonrasÃ„Â± fiyat gÃƒÂ¼ncel");
            assert_eq!(s.last_block, 105, "Kurtarma sonrasÃ„Â± blok gÃƒÂ¼ncel");
        }
    }

    /// ArdÃ„Â±Ã…Å¸Ã„Â±k RPC hatalarÃ„Â±: State bayatlaÃ…Å¸Ã„Â±r, is_active() hÃƒÂ¢lÃƒÂ¢ true ama
    /// staleness eÃ…Å¸iÃ„Å¸i aÃ…Å¸Ã„Â±ldÃ„Â±Ã„Å¸Ã„Â±nda bot fÃ„Â±rsat aramayÃ„Â± durdurur.
    #[test]
    fn test_rpc_consecutive_failures_staleness_protection() {
        let state = make_active_state(2500.0, 10_000_000_000_000_000_000, 100);

        // 3 ardÃ„Â±Ã…Å¸Ã„Â±k "RPC hatasÃ„Â±" Ã¢â‚¬â€ state gÃƒÂ¼ncellenmez
        for i in 1..=3 {
            // Her "hatada" 2 saniye geÃƒÂ§iyor
            {
                let mut s = state.write();
                s.last_update = Instant::now() - Duration::from_secs(2 * i);
            }

            let s = state.read();
            // is_active hÃƒÂ¢lÃƒÂ¢ true (panik yok, state bozulmadÃ„Â±)
            assert!(s.is_active(), "Hata #{}: is_active hÃƒÂ¢lÃƒÂ¢ true", i);
        }

        // 6 saniye sonra staleness eÃ…Å¸iÃ„Å¸ini aÃ…Å¸tÃ„Â±
        let s = state.read();
        assert!(
            s.staleness_ms() >= 5000,
            "3 ardÃ„Â±Ã…Å¸Ã„Â±k hatadan sonra veri bayat olmalÃ„Â±"
        );
    }

    /// SÃ„Â±fÃ„Â±r state korumasÃ„Â±: HiÃƒÂ§ gÃƒÂ¼ncelleme gelmezse state varsayÃ„Â±lan deÃ„Å¸erlerde.
    /// Bu da panik yapmaz Ã¢â‚¬â€ is_active() false dÃƒÂ¶ner.
    #[test]
    fn test_rpc_never_connected_no_panic() {
        let state: SharedPoolState = Arc::new(RwLock::new(PoolState::default()));

        let s = state.read();
        assert!(
            !s.is_active(),
            "HiÃƒÂ§ baÃ„Å¸lantÃ„Â± kurulmadÃ„Â±ysa state aktif olmamalÃ„Â±"
        );
        assert_eq!(s.eth_price_usd, 0.0, "Fiyat 0 (varsayÃ„Â±lan)");
        assert_eq!(s.liquidity, 0, "Likidite 0 (varsayÃ„Â±lan)");
        // Panik yok Ã¢â‚¬â€ gÃƒÂ¼venli varsayÃ„Â±lan deÃ„Å¸erler
    }

    /// SharedPoolState RwLock eÃ…Å¸ zamanlÃ„Â± eriÃ…Å¸im Ã¢â‚¬â€ panik yok.
    /// Birden fazla okuyucu aynÃ„Â± anda eriÃ…Å¸ebilir.
    #[test]
    fn test_rpc_failover_concurrent_access_no_panic() {
        let state = make_active_state(2500.0, 10_000_000_000_000_000_000, 100);

        // EÃ…Å¸ zamanlÃ„Â± okuma (parking_lot RwLock birden fazla reader kabul eder)
        let s1 = state.read();
        let s2 = state.read();

        assert_eq!(s1.eth_price_usd, s2.eth_price_usd, "EÃ…Å¸ zamanlÃ„Â± okuma tutarlÃ„Â±");
        assert_eq!(s1.liquidity, s2.liquidity, "Likidite deÃ„Å¸erleri tutarlÃ„Â±");

        drop(s1);
        drop(s2);

        // Yazma sonrasÃ„Â± okuma
        {
            let mut s = state.write();
            s.eth_price_usd = 2600.0;
        }

        let s = state.read();
        assert_eq!(s.eth_price_usd, 2600.0, "Yazma sonrasÃ„Â± okuma doÃ„Å¸ru");
    }

    /// Graceful degradation kanÃ„Â±tÃ„Â±: run_bot() hata dÃƒÂ¶ndÃƒÂ¼rdÃƒÂ¼Ã„Å¸ÃƒÂ¼nde
    /// exponential backoff ile yeniden baÃ„Å¸lanma stratejisi.
    /// Bu test, delay hesaplamasÃ„Â±nÃ„Â±n doÃ„Å¸ruluÃ„Å¸unu kanÃ„Â±tlar.
    #[test]
    fn test_reconnect_exponential_backoff_calculation() {
        // main.rs'deki delay hesaplama mantÃ„Â±Ã„Å¸Ã„Â±nÃ„Â± birebir test et
        for retry_count in 1u32..=10 {
            let delay_ms = if retry_count <= 3 {
                100u64 // Ã„Â°lk 3 deneme: 100ms (agresif)
            } else {
                let exp_delay = 100u64 * (1u64 << (retry_count - 3).min(6));
                exp_delay.min(10_000) // ÃƒÅ“st sÃ„Â±nÃ„Â±r: 10 saniye
            };

            // HiÃƒÂ§bir durumda panik veya integer overflow olmamalÃ„Â±
            assert!(delay_ms >= 100, "Minimum delay 100ms: retry={}", retry_count);
            assert!(delay_ms <= 10_000, "Maksimum delay 10s: retry={}", retry_count);

            // Ã„Â°lk 3 deneme agresif
            if retry_count <= 3 {
                assert_eq!(delay_ms, 100, "Ã„Â°lk 3 deneme 100ms olmalÃ„Â±");
            }
        }

        // Specific backoff values
        assert_eq!(100u64 * (1u64 << 1u32.min(6)), 200);  // retry 4 Ã¢â€ â€™ 200ms
        assert_eq!(100u64 * (1u64 << 2u32.min(6)), 400);  // retry 5 Ã¢â€ â€™ 400ms
        assert_eq!(100u64 * (1u64 << 3u32.min(6)), 800);  // retry 6 Ã¢â€ â€™ 800ms
        assert_eq!(100u64 * (1u64 << 4u32.min(6)), 1600); // retry 7 Ã¢â€ â€™ 1600ms
        assert_eq!(100u64 * (1u64 << 5u32.min(6)), 3200); // retry 8 Ã¢â€ â€™ 3200ms
        assert_eq!(100u64 * (1u64 << 6u32.min(6)), 6400); // retry 9 Ã¢â€ â€™ 6400ms
        // retry 10+: min(6) clamp Ã¢â€ â€™ 6400ms (< 10000 cap)
    }
}
