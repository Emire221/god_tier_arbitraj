// ============================================================================
//  EXECUTOR v20.0 Ã¢â‚¬â€ MEV KorumalÃ„Â± Ã„Â°Ã…Å¸lem GÃƒÂ¶nderimi (YalnÃ„Â±zca Private RPC)
//
//  Ãƒâ€“zellikler:
//  Ã¢Å“â€œ eth_sendBundle (Flashbots/Private RPC) ile sandwich korumasÃ„Â±
//  Ã¢Å“â€œ v20.0: PGA fallback TAMAMEN KALDIRILDI Ã¢â‚¬â€ L1 Data Fee kanamasÃ„Â± ÃƒÂ¶nlenir
//  Ã¢Å“â€œ Private RPC yoksa veya baÃ…Å¸arÃ„Â±sÃ„Â±zsa iÃ…Å¸lem Ã„Â°PTAL EDÃ„Â°LÃ„Â°R
//  Ã¢Å“â€œ Fire-and-forget receipt bekleme (pipeline bloke olmaz)
//  Ã¢Å“â€œ 4s timeout (Base 2s blok sÃƒÂ¼resi Ãƒâ€” 2)
//  Ã¢Å“â€œ Dinamik bribe hesabÃ„Â± (kÃƒÂ¢rÃ„Â±n %25'i validator'a tip)
//  Ã¢Å“â€œ Zero-copy calldata referanslarÃ„Â±
//  Ã¢Å“â€œ unwrap() yasak Ã¢â‚¬â€ tÃƒÂ¼m hatalar eyre ile yÃƒÂ¶netilir
// ============================================================================

use alloy::primitives::Address;
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use alloy::network::EthereumWallet;
use eyre::Result;
use serde::Serialize;
use std::sync::Arc;

use crate::types::*;

// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
// MEV Bundle YapÃ„Â±larÃ„Â± (Flashbots uyumlu JSON-RPC)
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

/// eth_sendBundle isteÃ„Å¸i (Flashbots / MEV-Share / Private RPC uyumlu)
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BundleRequest {
    /// Ã„Â°mzalanmÃ„Â±Ã…Å¸ raw transaction listesi (hex encoded)
    txs: Vec<String>,
    /// Hedef blok numarasÃ„Â± (hex)
    block_number: String,
    /// Opsiyonel: Minimum timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    min_timestamp: Option<u64>,
    /// Opsiyonel: Maksimum timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    max_timestamp: Option<u64>,
}


// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
// MEV KorumalÃ„Â± Executor
// Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

/// MEV-korumalÃ„Â± iÃ…Å¸lem yÃƒÂ¼rÃƒÂ¼tÃƒÂ¼cÃƒÂ¼sÃƒÂ¼ (YalnÃ„Â±zca Private RPC).
///
/// Ã„Â°Ã…Å¸lem gÃƒÂ¶nderme stratejisi:
///   1. Private/Flashbots RPC varsa Ã¢â€ â€™ eth_sendBundle dene
///   2. Bundle baÃ…Å¸arÃ„Â±sÃ„Â±zsa Ã¢â€ â€™ Ã„Â°Ã…Å¸lem Ã„Â°PTAL (v20.0: PGA fallback kaldÃ„Â±rÃ„Â±ldÃ„Â±)
///   3. Private RPC yoksa Ã¢â€ â€™ Ã„Â°Ã…Å¸lem Ã„Â°PTAL (gÃƒÂ¶nderilmez)
///
/// v20.0 Kritik DeÃ„Å¸iÃ…Å¸iklik:
///   PGA (Public Mempool) fallback tamamen kaldÃ„Â±rÃ„Â±ldÃ„Â±.
///   L2 aÃ„Å¸larÃ„Â±nda (Base) public mempool'da revert olan iÃ…Å¸lemler
///   gas ÃƒÂ¶demese dahi L1 Data Fee ÃƒÂ¶demek zorundadÃ„Â±r.
///   Bu durum cÃƒÂ¼zdanÃ„Â±n sÃƒÂ¼rekli L1 ÃƒÂ¼cretleriyle kanamasÃ„Â±na yol aÃƒÂ§Ã„Â±yordu.
///
/// Bribe (validator tip) hesabÃ„Â±:
///   - KÃƒÂ¢rÃ„Â±n dinamik yÃƒÂ¼zdesi (%25 base, margin'e gÃƒÂ¶re uyarlanÃ„Â±r)
///   - Priority fee olarak TX'e eklenir
///   - Base L2 FIFO: priority fee sÃ„Â±ralama belirler
pub struct MevExecutor {
    /// Private/Flashbots RPC URL (eth_sendBundle iÃƒÂ§in)
    /// Ãƒâ€“rn: https://relay.flashbots.net veya ÃƒÂ¶zel builder endpoint
    private_rpc_url: Option<String>,
    /// Standart RPC URL (v23.0: receipt polling private RPC'ye taÃ…Å¸Ã„Â±ndÃ„Â±)
        standard_rpc_url: String,
    /// Dinamik bribe yÃƒÂ¼zde tabanÃ„Â± (0.25 = %25)
    base_bribe_pct: f64,
}

impl MevExecutor {
    /// Yeni MEV Executor oluÃ…Å¸tur.
    ///
    /// # ArgÃƒÂ¼manlar
    /// - `private_rpc_url`: Flashbots/Private RPC URL (None ise fallback)
    /// - `standard_rpc_url`: Normal RPC URL
    /// - `base_bribe_pct`: KÃƒÂ¢r bribe yÃƒÂ¼zdesi (0.25 = %25)
    pub fn new(
        private_rpc_url: Option<String>,
        standard_rpc_url: String,
        base_bribe_pct: f64,
    ) -> Self {
        Self {
            private_rpc_url,
            standard_rpc_url,
            base_bribe_pct,
        }
    }

    /// v25.0: Standart RPC URL'sine eriÃ…Å¸im (whitelist TX gÃƒÂ¶ndermek iÃƒÂ§in)
    pub fn standard_rpc_url(&self) -> &str {
        &self.standard_rpc_url
    }

    /// Ã„Â°Ã…Å¸lemi MEV-korumalÃ„Â± olarak gÃƒÂ¶nder.
    ///
    /// # AkÃ„Â±Ã…Å¸
    /// 1. TX oluÃ…Å¸tur (calldata + dinamik bribe priority fee)
    /// 2. TX'i imzala
    /// 3. Private RPC varsa Ã¢â€ â€™ eth_sendBundle
    /// 4. Private RPC yoksa veya baÃ…Å¸arÃ„Â±sÃ„Â±zsa Ã¢â€ â€™ Ã„Â°Ã…Å¸lem Ã„Â°PTAL EDÃ„Â°LÃ„Â°R
    ///
    /// v20.0 KRÃ„Â°TÃ„Â°K DEÃ„ÂÃ„Â°Ã…ÂÃ„Â°KLÃ„Â°K: PGA (Public Mempool) fallback TAMAMEN KALDIRILDI.
    /// L2 aÃ„Å¸larÃ„Â±nda (Base) public mempool'da sandviÃƒÂ§ yiyen revert iÃ…Å¸lemleri
    /// gas ÃƒÂ¶demese dahi L1 Data Fee ÃƒÂ¶demek zorundadÃ„Â±r. Bu durum cÃƒÂ¼zdanÃ„Â±n
    /// sÃƒÂ¼rekli L1 ÃƒÂ¼cretleri ile kanamasÃ„Â±na yol aÃƒÂ§Ã„Â±yordu.
    /// ArtÃ„Â±k Private RPC baÃ…Å¸arÃ„Â±sÃ„Â±z olursa iÃ…Å¸lem iptal edilir.
    pub async fn execute_protected(
        &self,
        private_key: &str,
        contract_address: Address,
        calldata: &[u8],
        nonce: u64,
        expected_profit_weth: f64,
        simulated_gas: u64,
        block_base_fee: u64,
        current_block: u64,
        nonce_manager: &Arc<NonceManager>,
    ) -> Result<String> {
        // 1. Dinamik bribe hesabÃ„Â±
        let bribe_info = self.compute_dynamic_bribe(
            expected_profit_weth,
            simulated_gas,
            block_base_fee,
        );

        // 2. TX oluÃ…Å¸tur
        let gas_limit = ((simulated_gas as f64) * 1.10) as u128;
        let gas_limit = gas_limit.max(100_000);

        let max_fee = {
            let base_component = (block_base_fee as u128).saturating_mul(2);
            let priority_component = bribe_info.priority_fee_per_gas;
            base_component
                .saturating_add(priority_component)
                .max(1_000_000_000) // Min 1 Gwei
        };

        let tx = TransactionRequest::default()
            .to(contract_address)
            .input(alloy::primitives::Bytes::copy_from_slice(calldata).into())
            .nonce(nonce)
            .gas_limit(gas_limit)
            .max_fee_per_gas(max_fee)
            .max_priority_fee_per_gas(bribe_info.priority_fee_per_gas);

        eprintln!(
            "     ÄŸÅ¸â€™Â° MEV Bribe: {:.0}% (marj: {:.1}x, priority: {} Gwei, profit: {:.6} WETH)",
            bribe_info.effective_pct * 100.0,
            bribe_info.profit_margin_ratio,
            bribe_info.priority_fee_per_gas / 1_000_000_000,
            expected_profit_weth,
        );

        // 3. Ã„Â°mzala
        let signer: PrivateKeySigner = private_key
            .parse()
            .map_err(|_| eyre::eyre!("GeÃƒÂ§ersiz private key"))?;
        let wallet = EthereumWallet::from(signer.clone());

        // 4. GÃƒÂ¶nder Ã¢â‚¬â€ YALNIZCA Private RPC. PGA fallback v20.0'da kaldÃ„Â±rÃ„Â±ldÃ„Â±.
        //
        // v20.0: L2 aÃ„Å¸larÃ„Â±nda (Base) public mempool'a dÃƒÂ¼Ã…Å¸en iÃ…Å¸lemler:
        //   - SandviÃƒÂ§ saldÃ„Â±rÃ„Â±sÃ„Â±na aÃƒÂ§Ã„Â±ktÃ„Â±r
        //   - Revert olsa bile L1 Data Fee ÃƒÂ¶demek zorundadÃ„Â±r (~0.001-0.01 ETH)
        //   - Bu durum cÃƒÂ¼zdanÃ„Â±n sÃƒÂ¼rekli L1 ÃƒÂ¼cretleri ile kanamasÃ„Â±na yol aÃƒÂ§ar
        //
        // Ãƒâ€¡ÃƒÂ¶zÃƒÂ¼m: Private RPC yoksa veya baÃ…Å¸arÃ„Â±sÃ„Â±zsa iÃ…Å¸lem Ã„Â°PTAL EDÃ„Â°LÃ„Â°R.
        if let Some(ref private_url) = self.private_rpc_url {
            match self.send_bundle(
                private_url,
                &wallet,
                &signer,
                tx.clone(),
                current_block,
                nonce_manager,
            ).await {
                Ok(hash) => Ok(hash),
                Err(e) => {
                    // v22.1: rollback kaldÃ„Â±rÃ„Â±ldÃ„Â± Ã¢â‚¬â€ race condition riski.
                    // Periyodik nonce sync (50 blokta bir) yeterli.
                    eprintln!(
                        "     Ã¢ÂÅ’ [v20.0] Private RPC bundle baÃ…Å¸arÃ„Â±sÃ„Â±z Ã¢â‚¬â€ iÃ…Å¸lem Ã„Â°PTAL EDÃ„Â°LDÃ„Â°: {}",
                        e
                    );
                    eprintln!(
                        "     Ã¢â€ºâ€ [v20.0] PGA fallback devre dÃ„Â±Ã…Å¸Ã„Â± Ã¢â‚¬â€ L1 Data Fee kanamasÃ„Â± ÃƒÂ¶nlendi"
                    );
                    Err(eyre::eyre!("Private RPC bundle baÃ…Å¸arÃ„Â±sÃ„Â±z, PGA fallback devre dÃ„Â±Ã…Å¸Ã„Â±: {}", e))
                }
            }
        } else {
            // v22.1: rollback kaldÃ„Â±rÃ„Â±ldÃ„Â± Ã¢â‚¬â€ race condition riski.
            eprintln!(
                "     Ã¢ÂÅ’ [v20.0] PRIVATE_RPC_URL tanÃ„Â±mlÃ„Â± deÃ„Å¸il Ã¢â‚¬â€ iÃ…Å¸lem Ã„Â°PTAL EDÃ„Â°LDÃ„Â°"
            );
            eprintln!(
                "     Ã¢â€ºâ€ [v20.0] Public mempool gÃƒÂ¶nderimi devre dÃ„Â±Ã…Å¸Ã„Â± Ã¢â‚¬â€ L1 Data Fee kanamasÃ„Â± ÃƒÂ¶nlendi"
            );
            Err(eyre::eyre!("Private RPC URL tanÃ„Â±mlÃ„Â± deÃ„Å¸il. GÃƒÂ¼venlik nedeniyle public mempool'a gÃƒÂ¶nderilmez."))
        }
    }

    // v20.0: send_pga_fallback() KALDIRILDI Ã¢â‚¬â€ public mempool gÃƒÂ¶nderimi
    // L2 aÃ„Å¸larÃ„Â±nda L1 Data Fee kanama riski oluÃ…Å¸turuyordu.
    // TÃƒÂ¼m iÃ…Å¸lemler yalnÃ„Â±zca Private RPC (eth_sendBundle) ÃƒÂ¼zerinden gÃƒÂ¶nderilir.
    // Bundle baÃ…Å¸arÃ„Â±sÃ„Â±z olursa Ã¢â€ â€™ iÃ…Å¸lem iptal edilir.
    // v22.1: nonce rollback kaldÃ„Â±rÃ„Â±ldÃ„Â± Ã¢â‚¬â€ race condition riski.
    // Periyodik nonce sync (50 blokta bir) nonce tutarlÃ„Â±lÃ„Â±Ã„Å¸Ã„Â±nÃ„Â± saÃ„Å¸lar.

    /// eth_sendBundle ile Flashbots/Private builder'a gÃƒÂ¶nder.
    ///
    /// v22.1 KRÃ„Â°TÃ„Â°K DÃƒÅ“ZELTME:
    ///   - TX artÃ„Â±k public mempool'a GÃƒâ€“NDERÃ„Â°LMEZ
    ///   - TX, on_http(private_rpc_url) ile YALNIZCA private endpoint'e gider
    ///   - Ek olarak eth_sendBundle ile aynÃ„Â± private RPC'ye bundle POST edilir
    ///   - Standard RPC'ye HÃ„Â°Ãƒâ€¡BÃ„Â°R TX gÃƒÂ¶nderilmez
    async fn send_bundle(
        &self,
        private_rpc_url: &str,
        wallet: &EthereumWallet,
        signer: &PrivateKeySigner,
        tx: TransactionRequest,
        current_block: u64,
        _nonce_manager: &Arc<NonceManager>,
    ) -> Result<String> {
        let target_block = current_block + 1;
        let target_block_hex = format!("0x{:x}", target_block);

        // v23.0 KRÃ„Â°TÃ„Â°K DÃƒÅ“ZELTME (K-1): Bundle txs alanÃ„Â±na raw signed TX konulur.
        // Eski: provider.send_transaction() ile hash alÃ„Â±nÃ„Â±p txs'e konuyordu Ã¢â‚¬â€ HATALI.
        // eth_sendBundle spec'i txs alanÃ„Â±nda raw signed TX hex'i ister, hash deÃ„Å¸il.
        //
        // Yeni akÃ„Â±Ã…Å¸:
        //   1. TX'i private RPC ÃƒÂ¼zerinden gÃƒÂ¶nder (imzala + send)
        //   2. AynÃ„Â± TX'i raw hex olarak al
        //   3. Bundle txs alanÃ„Â±na raw hex koy
        //   4. Bundle'Ã„Â± private RPC'ye POST et
        let private_url: reqwest::Url = private_rpc_url.parse()
            .map_err(|e| eyre::eyre!("Private RPC URL parse hatasÃ„Â±: {}", e))?;
        let provider = ProviderBuilder::new()
            .with_recommended_fillers()
            .wallet(wallet.clone())
            .on_http(private_url);

        // TX'i private RPC ÃƒÂ¼zerinden gÃƒÂ¶nder (imzala + eth_sendRawTransaction)
        // TX yalnÃ„Â±zca private endpoint'e ulaÃ…Å¸Ã„Â±r, public mempool'a DÃƒÅ“Ã…ÂMEZ
        let pending = provider.send_transaction(tx.clone())
            .await
            .map_err(|e| eyre::eyre!("Private RPC TX gÃƒÂ¶nderim hatasÃ„Â±: {}", e))?;

        let tx_hash = format!("{:?}", pending.tx_hash());
        let tx_hash_alloy = *pending.tx_hash();
        drop(pending);

        // v23.0 (K-1): raw signed TX'i oluÃ…Å¸tur ve RLP encode et.
        // EIP-1559 TX oluÃ…Å¸tur, PrivateKeySigner ile imzala, raw bytes al.
        let raw_tx_hex = {
            use alloy::consensus::{TxEip1559, SignableTransaction, TxEnvelope};
            use alloy::signers::Signer;

            let input_data = tx.input.input().cloned().unwrap_or_default();
            // to alanÃ„Â± Ã¢â‚¬â€ TransactionRequest'teki TxKind'dan Address ÃƒÂ§Ã„Â±kar
            let to_addr = match tx.to {
                Some(alloy::primitives::TxKind::Call(addr)) => alloy::primitives::TxKind::Call(addr),
                other => other.unwrap_or(alloy::primitives::TxKind::Create),
            };

            let eip1559_tx = TxEip1559 {
                chain_id: 8453, // Base chain ID
                nonce: tx.nonce.unwrap_or(0),
                gas_limit: tx.gas.unwrap_or(100_000),
                max_fee_per_gas: tx.max_fee_per_gas.unwrap_or(1_000_000_000),
                max_priority_fee_per_gas: tx.max_priority_fee_per_gas.unwrap_or(1_000_000),
                to: to_addr,
                input: input_data,
                ..Default::default()
            };

            match signer.sign_hash(&eip1559_tx.signature_hash()).await {
                Ok(signature) => {
                    let signed = eip1559_tx.into_signed(signature);
                    let envelope = TxEnvelope::Eip1559(signed);
                    let raw_bytes = alloy::eips::eip2718::Encodable2718::encoded_2718(&envelope);
                    format!("0x{}", alloy::primitives::hex::encode(&raw_bytes))
                }
                Err(e) => {
                    // Ã„Â°mzalama baÃ…Å¸arÃ„Â±sÃ„Â±z olursa TX hash'ini kullan (degraded mode)
                    eprintln!("     Ã¢Å¡Â Ã¯Â¸Â  Raw TX oluÃ…Å¸turma hatasÃ„Â± (bundle degraded mode): {}", e);
                    tx_hash.clone()
                }
            }
        };

        // v23.0: Bundle txs alanÃ„Â±na raw signed TX hex konulur (hash deÃ„Å¸il!)
        let bundle = BundleRequest {
            txs: vec![raw_tx_hex.clone()],
            block_number: target_block_hex.clone(),
            min_timestamp: None,
            max_timestamp: None,
        };

        let bundle_json = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_sendBundle",
            "params": [bundle]
        });

        // HTTP POST ile private RPC'ye gÃƒÂ¶nder
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .map_err(|e| eyre::eyre!("HTTP client oluÃ…Å¸turma hatasÃ„Â±: {}", e))?;

        match http_client
            .post(private_rpc_url)
            .header("Content-Type", "application/json")
            .json(&bundle_json)
            .send()
            .await
        {
            Ok(response) => {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                if !status.is_success() {
                    eprintln!(
                        "     Ã¢Å¡Â Ã¯Â¸Â  Private RPC yanÃ„Â±t hatasÃ„Â± (HTTP {}): {}",
                        status, &body[..body.len().min(200)]
                    );
                } else if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&body) {
                    if let Some(error) = parsed.get("error") {
                        eprintln!("     Ã¢Å¡Â Ã¯Â¸Â  eth_sendBundle RPC hatasÃ„Â±: {}", error);
                    }
                }
            }
            Err(e) => {
                eprintln!("     Ã¢Å¡Â Ã¯Â¸Â  Private RPC HTTP POST hatasÃ„Â±: {}", e);
            }
        }

        eprintln!(
            "     ÄŸÅ¸â€œÂ¦ Bundle gÃƒÂ¶nderildi Ã¢â€ â€™ blok #{} | private RPC: {}",
            target_block,
            &private_rpc_url[..private_rpc_url.len().min(40)]
        );

        // Sonraki blok iÃƒÂ§in de gÃƒÂ¶nder (dÃƒÂ¼Ã…Å¸me ihtimaline karÃ…Å¸Ã„Â±)
        let next_target_hex = format!("0x{:x}", target_block + 1);
        let next_bundle = BundleRequest {
            txs: vec![raw_tx_hex],
            block_number: next_target_hex,
            min_timestamp: None,
            max_timestamp: None,
        };

        let next_bundle_json = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "eth_sendBundle",
            "params": [next_bundle]
        });

        // Yedek bundle'Ã„Â± da gÃƒÂ¶nder (hata kritik deÃ„Å¸il)
        let _ = http_client
            .post(private_rpc_url)
            .header("Content-Type", "application/json")
            .json(&next_bundle_json)
            .send()
            .await;

        eprintln!("     ÄŸÅ¸â€œÂ¦ Yedek bundle Ã¢â€ â€™ blok #{}", target_block + 1);

        // Fire-and-forget: Receipt bekleme arka plana taÃ…Å¸Ã„Â±nÃ„Â±r
        // v22.0: Timeout 4s Ã¢â€ â€™ 10s (5 blok). Nonce rollback kaldÃ„Â±rÃ„Â±ldÃ„Â± Ã¢â‚¬â€
        // periyodik nonce sync (50 blokta bir) yeterli, race condition ÃƒÂ¶nlenir.
        // v23.0 (Y-2): Receipt polling private RPC ÃƒÂ¼zerinden yapÃ„Â±lÃ„Â±r.
        // Standard RPC kullanmak TX sÃ„Â±zÃ„Â±ntÃ„Â±sÃ„Â±na yol aÃƒÂ§abilir.
        let rpc_url_clone = private_rpc_url.to_string();
        let hash_clone = tx_hash.clone();
        tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
            // v23.0 (Y-2): Receipt polling private RPC HTTP ÃƒÂ¼zerinden yapÃ„Â±lÃ„Â±r
            let poll_url: reqwest::Url = match rpc_url_clone.parse() {
                Ok(u) => u,
                Err(e) => {
                    eprintln!("     Ã¢Å¡Â Ã¯Â¸Â  Receipt polling URL parse hatasÃ„Â±: {}", e);
                    return;
                }
            };
            let poll_provider = ProviderBuilder::new().on_http(poll_url);
            loop {
                if tokio::time::Instant::now() > deadline {
                    eprintln!("     Ã¢ÂÂ° Bundle timeout (10s) Ã¢â‚¬â€ TX dahil edilmemiÃ…Å¸ olabilir: {}", &hash_clone);
                    break;
                }
                match poll_provider.get_transaction_receipt(tx_hash_alloy).await {
                    Ok(Some(receipt)) => {
                        eprintln!(
                            "     Ã¢Å“â€¦ Bundle dahil edildi: blok #{}",
                            receipt.block_number.unwrap_or_default()
                        );
                        break;
                    }
                    Ok(None) => {
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    }
                    Err(e) => {
                        eprintln!("     Ã¢Å¡Â Ã¯Â¸Â  Bundle receipt hatasÃ„Â±: {}", e);
                        break;
                    }
                }
            }
        });

        Ok(tx_hash)
    }

    // Ã¢â€â‚¬Ã¢â€â‚¬ PGA Fallback GÃƒÂ¼venlik Notu (v20.0) Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
    // v20.0: PGA (Public Mempool) fallback TAMAMEN KALDIRILDI.
    //
    // L2 aÃ„Å¸larÃ„Â±nda (Base, OP Stack) public mempool riski:
    //   - SandviÃƒÂ§ saldÃ„Â±rÃ„Â±sÃ„Â±na aÃƒÂ§Ã„Â±k TX'ler
    //   - Revert olsa bile L1 Data Fee ÃƒÂ¶denmek zorunda (~0.001-0.01 ETH)
    //   - SÃƒÂ¼rekli revert + L1 fee = cÃƒÂ¼zdan kanamasÃ„Â±
    //
    // Ãƒâ€¡ÃƒÂ¶zÃƒÂ¼m: Bot, Private/Flashbots RPC olmadan ASLA iÃ…Å¸lem gÃƒÂ¶ndermez.
    // send_pga_fallback() fonksiyonu kaldÃ„Â±rÃ„Â±ldÃ„Â±.
    // Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬
    // Ã¢â€â‚¬Ã¢â€â‚¬ Dinamik Bribe HesabÃ„Â± Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

    /// Bribe hesaplama sonucu
    pub fn compute_dynamic_bribe(
        &self,
        expected_profit_weth: f64,
        simulated_gas: u64,
        block_base_fee: u64,
    ) -> BribeInfo {
        let _expected_profit_wei = safe_f64_to_u128(expected_profit_weth * 1e18);

        // Gas maliyeti (WETH cinsinden)
        let gas_cost_weth = (simulated_gas as f64 * block_base_fee as f64) / 1e18;

        // KÃƒÂ¢r/Gas oranÃ„Â±
        let profit_margin_ratio = if gas_cost_weth > 0.00001 {
            expected_profit_weth / gas_cost_weth
        } else {
            10.0
        };

        // v24.0: Agresif PGA modÃƒÂ¼lÃƒÂ¼ Ã¢â‚¬â€ %10 ile %95 aralÃ„Â±Ã„Å¸Ã„Â±nda dinamik bribe.
        //
        // Base L2 sequencer sÃ„Â±ralamasÃ„Â± yalnÃ„Â±zca priority fee ile belirlenir.
        // RekabetÃƒÂ§i bloklarda rakip botlar kÃƒÂ¢rÃ„Â±n %99'una kadar rÃƒÂ¼Ã…Å¸vet
        // teklif edebilir. DÃƒÂ¼Ã…Å¸ÃƒÂ¼k marjlÃ„Â± fÃ„Â±rsatlarda agresif olmak gerekir.
        //
        // Yeni kademeler:
        //   margin >= 10x Ã¢â€ â€™ %10 (ÃƒÂ§ok dÃƒÂ¼Ã…Å¸ÃƒÂ¼k rekabet, kÃƒÂ¢rÃ„Â± koru)
        //   margin 5-10x  Ã¢â€ â€™ %25 (dÃƒÂ¼Ã…Å¸ÃƒÂ¼k rekabet)
        //   margin 3-5x   Ã¢â€ â€™ %40 (orta rekabet)
        //   margin 2-3x   Ã¢â€ â€™ %60 (yÃƒÂ¼ksek rekabet)
        //   margin 1.5-2x Ã¢â€ â€™ %80 (ÃƒÂ§ok yÃƒÂ¼ksek rekabet)
        //   margin < 1.5x Ã¢â€ â€™ %95 (maksimum agresiflik Ã¢â‚¬â€ rakipleri ez)
        let effective_pct = if profit_margin_ratio >= 10.0 {
            self.base_bribe_pct.max(0.10)
        } else if profit_margin_ratio >= 5.0 {
            0.25
        } else if profit_margin_ratio >= 3.0 {
            0.40
        } else if profit_margin_ratio >= 2.0 {
            0.60
        } else if profit_margin_ratio >= 1.5 {
            0.80
        } else {
            0.95 // v24.0: Eski %70 Ã¢â€ â€™ %95 (maksimum rekabet gÃƒÂ¼cÃƒÂ¼)
        };

        // v20.0: Minimum mutlak kÃƒÂ¢r korumasÃ„Â±
        // Bribe sonrasÃ„Â± kalan kÃƒÂ¢r en az 0.0001 WETH (~$0.25) olmalÃ„Â±.
        // Bu, L1 Data Fee dalgalanmasÃ„Â±nÃ„Â± karÃ…Å¸Ã„Â±layacak statik gÃƒÂ¼venlik marjÃ„Â±dÃ„Â±r.
        let min_absolute_profit_weth: f64 = 0.0001;
        let max_bribe_weth = (expected_profit_weth - gas_cost_weth - min_absolute_profit_weth).max(0.0);
        let computed_bribe_weth = expected_profit_weth * effective_pct;
        let actual_bribe_weth = computed_bribe_weth.min(max_bribe_weth);
        let actual_effective_pct = if expected_profit_weth > 0.0 {
            actual_bribe_weth / expected_profit_weth
        } else {
            0.0
        };

        // Bribe wei
        let bribe_wei = safe_f64_to_u128(actual_bribe_weth * 1e18);

        // Priority fee per gas
        let gas_with_buffer = safe_f64_to_u128((simulated_gas as f64) * 1.10);
        let actual_gas = gas_with_buffer.max(100_000);
        let priority_fee = if actual_gas > 0 {
            (bribe_wei / actual_gas).max(1_000_000) // Min 1 Mwei
        } else {
            1_000_000_000 // Fallback 1 Gwei
        };

        BribeInfo {
            bribe_wei,
            priority_fee_per_gas: priority_fee,
            effective_pct: actual_effective_pct,
            profit_margin_ratio,
            gas_cost_weth,
        }
    }
}

/// Bribe hesaplama sonucu
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct BribeInfo {
    /// Toplam bribe miktarÃ„Â± (wei)
    pub bribe_wei: u128,
    /// Gas baÃ…Å¸Ã„Â±na priority fee (wei)
    pub priority_fee_per_gas: u128,
    /// Uygulanan efektif bribe yÃƒÂ¼zdesi
    pub effective_pct: f64,
    /// KÃƒÂ¢r/gas marj oranÃ„Â±
    pub profit_margin_ratio: f64,
    /// Gas maliyeti (WETH)
    pub gas_cost_weth: f64,
}

/// Yeni keÃ…Å¸fedilen havuzlarÃ„Â± on-chain whiteliste eklemek iÃƒÂ§in ABI-encoded calldata oluÃ…Å¸tur.
///
/// Format: executorBatchAddPools(address[])
///   selector(4) + offset(32) + length(32) + addresses(N*32)
pub fn encode_whitelist_calldata(pool_addresses: &[Address]) -> Vec<u8> {
    use alloy::sol_types::SolCall;
    use alloy::sol;

    sol! {
        function executorBatchAddPools(address[] pools);
    }

    let call = executorBatchAddPoolsCall {
        pools: pool_addresses.to_vec(),
    };
    executorBatchAddPoolsCall::abi_encode(&call)
}
