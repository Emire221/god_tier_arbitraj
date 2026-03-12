// ============================================================================
//  EXECUTOR v25.0 — Base L2 Private RPC İşlem Gönderimi
//
//  Özellikler:
//  ✓ eth_sendRawTransaction (Private RPC endpoint) ile sandwich koruması
//  ✓ v25.0: eth_sendBundle KALDIRILDI — Base L2'de Flashbots builder YOK
//  ✓ Private RPC yoksa işlem İPTAL EDİLİR
//  ✓ Fire-and-forget receipt bekleme (pipeline bloke olmaz)
//  ✓ 10s timeout (5 blok Base L2)
//  ✓ Dinamik bribe hesabı (kârın %25'i priority fee olarak)
//  ✓ Zero-copy calldata referansları
//  ✓ unwrap() yasak — tüm hatalar eyre ile yönetilir
// ============================================================================

use alloy::primitives::Address;
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use alloy::network::EthereumWallet;
use eyre::Result;
use std::sync::Arc;

use crate::types::*;

// ─────────────────────────────────────────────────────────────────────────────
// MEV Korumalı Executor
// ─────────────────────────────────────────────────────────────────────────────

/// MEV-korumalı işlem yürütücüsü (Yalnızca Private RPC).
///
/// v25.0 Kritik Değişiklik:
///   Base L2 (Coinbase Sequencer) geleneksel Flashbots Builder yapısını
///   desteklemez. eth_sendBundle işlemleri sessizce yutulur/reddedilir.
///   Bunun yerine, Private RPC endpoint'ine (ör: Flashbots Protect)
///   eth_sendRawTransaction ile EIP-1559 TX gönderilir.
///
/// İşlem gönderme stratejisi:
///   1. Private RPC varsa → eth_sendRawTransaction (gizli endpoint)
///   2. Private RPC yoksa → İşlem İPTAL (gönderilmez)
///
/// Bribe (validator tip) hesabı:
///   - Kârın dinamik yüzdesi (%25 base, margin'e göre uyarlanır)
///   - Priority fee olarak TX'e eklenir
///   - Base L2 FIFO: priority fee sıralama belirler
pub struct MevExecutor {
    /// Private RPC URL (eth_sendRawTransaction için)
    /// Örn: https://rpc.flashbots.net/fast?chainId=8453
    private_rpc_url: Option<String>,
    /// Standart RPC URL (whitelist TX, receipt polling)
    standard_rpc_url: String,
    /// Dinamik bribe yüzde tabanı (0.25 = %25)
    base_bribe_pct: f64,
}

impl MevExecutor {
    /// Yeni MEV Executor oluştur.
    ///
    /// # Argümanlar
    /// - `private_rpc_url`: Flashbots/Private RPC URL (None ise fallback)
    /// - `standard_rpc_url`: Normal RPC URL
    /// - `base_bribe_pct`: Kâr bribe yüzdesi (0.25 = %25)
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

    /// v25.0: Standart RPC URL'sine erişim (whitelist TX göndermek için)
    pub fn standard_rpc_url(&self) -> &str {
        &self.standard_rpc_url
    }

    /// İşlemi MEV-korumalı olarak gönder.
    ///
    /// # Akış
    /// 1. TX oluştur (calldata + dinamik bribe priority fee)
    /// 2. TX'i imzala
    /// 3. Private RPC varsa → eth_sendRawTransaction
    /// 4. Private RPC yoksa veya başarısızsa → İşlem İPTAL EDİLİR
    ///
    /// v20.0 KRİTİK DEĞİŞİKLİK: PGA (Public Mempool) fallback TAMAMEN KALDIRILDI.
    /// L2 ağlarında (Base) public mempool'da sandviç yiyen revert işlemleri
    /// gas ödemese dahi L1 Data Fee ödemek zorundadır. Bu durum cüzdanın
    /// sürekli L1 ücretleri ile kanamasına yol açıyordu.
    /// Artık Private RPC başarısız olursa işlem iptal edilir.
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
        _nonce_manager: &Arc<NonceManager>,
    ) -> Result<String> {
        // 1. Dinamik bribe hesabı
        let bribe_info = self.compute_dynamic_bribe(
            expected_profit_weth,
            simulated_gas,
            block_base_fee,
        );

        // 2. TX oluştur
        let gas_limit = ((simulated_gas as f64) * 1.10) as u64;
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
            "     💰 MEV Bribe: {:.0}% (marj: {:.1}x, priority: {} Gwei, profit: {:.6} WETH)",
            bribe_info.effective_pct * 100.0,
            bribe_info.profit_margin_ratio,
            bribe_info.priority_fee_per_gas / 1_000_000_000,
            expected_profit_weth,
        );

        // 3. İmzala
        let signer: PrivateKeySigner = private_key
            .parse()
            .map_err(|_| eyre::eyre!("Geçersiz private key"))?;
        let wallet = EthereumWallet::from(signer.clone());

        // 4. Gönder — YALNIZCA Private RPC (eth_sendRawTransaction).
        //
        // v25.0: Base L2'de Flashbots builder yapısı yoktur.
        // İşlemler Private RPC endpoint'ine eth_sendRawTransaction ile gönderilir.
        // Private RPC yoksa işlem İPTAL EDİLİR.
        if let Some(ref private_url) = self.private_rpc_url {
            match self.send_private_tx(
                private_url,
                &wallet,
                tx.clone(),
                current_block,
            ).await {
                Ok(hash) => Ok(hash),
                Err(e) => {
                    eprintln!(
                        "     ❌ [v25.0] Private RPC TX başarısız — işlem İPTAL EDİLDİ: {}",
                        e
                    );
                    Err(eyre::eyre!("Private RPC TX başarısız: {}", e))
                }
            }
        } else {
            eprintln!(
                "     ❌ [v25.0] PRIVATE_RPC_URL tanımlı değil — işlem İPTAL EDİLDİ"
            );
            Err(eyre::eyre!("Private RPC URL tanımlı değil. Güvenlik nedeniyle public mempool'a gönderilmez."))
        }
    }

    /// eth_sendRawTransaction ile Private RPC endpoint'ine gönder.
    ///
    /// v25.0: Base L2 (Coinbase Sequencer) eth_sendBundle desteklemez.
    /// Bunun yerine, EIP-1559 formatında imzalanmış TX, private endpoint'e
    /// (ör: https://rpc.flashbots.net/fast?chainId=8453) standart
    /// eth_sendRawTransaction metodu ile POST edilir.
    ///
    /// TX yalnızca private endpoint'e ulaşır, public mempool'a DÜŞMEZ.
    async fn send_private_tx(
        &self,
        private_rpc_url: &str,
        wallet: &EthereumWallet,
        tx: TransactionRequest,
        current_block: u64,
    ) -> Result<String> {
        let private_url: reqwest::Url = private_rpc_url.parse()
            .map_err(|e| eyre::eyre!("Private RPC URL parse hatası: {}", e))?;
        let provider = ProviderBuilder::new()
            .wallet(wallet.clone())
            .connect_http(private_url);

        // TX'i private RPC üzerinden gönder (imzala + eth_sendRawTransaction)
        // TX yalnızca private endpoint'e ulaşır, public mempool'a DÜŞMEZ
        let pending = provider.send_transaction(tx)
            .await
            .map_err(|e| eyre::eyre!("Private RPC TX gönderim hatası: {}", e))?;

        let tx_hash = format!("{:?}", pending.tx_hash());
        let tx_hash_alloy = *pending.tx_hash();
        drop(pending);

        eprintln!(
            "     📤 TX gönderildi → blok #{} | private RPC: {}",
            current_block + 1,
            &private_rpc_url[..private_rpc_url.len().min(50)]
        );

        // Fire-and-forget: Receipt bekleme arka plana taşınır
        let rpc_url_clone = private_rpc_url.to_string();
        let hash_clone = tx_hash.clone();
        tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
            let poll_url: reqwest::Url = match rpc_url_clone.parse() {
                Ok(u) => u,
                Err(e) => {
                    eprintln!("     ⚠️  Receipt polling URL parse hatası: {}", e);
                    return;
                }
            };
            let poll_provider = ProviderBuilder::new().connect_http(poll_url);
            loop {
                if tokio::time::Instant::now() > deadline {
                    eprintln!("     ⏰ TX timeout (10s) — dahil edilmemiş olabilir: {}", &hash_clone);
                    break;
                }
                match poll_provider.get_transaction_receipt(tx_hash_alloy).await {
                    Ok(Some(receipt)) => {
                        eprintln!(
                            "     ✅ TX dahil edildi: blok #{}",
                            receipt.block_number.unwrap_or_default()
                        );
                        break;
                    }
                    Ok(None) => {
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    }
                    Err(e) => {
                        eprintln!("     ⚠️  TX receipt hatası: {}", e);
                        break;
                    }
                }
            }
        });

        Ok(tx_hash)
    }

    // ── Dinamik Bribe Hesabı ─────────────────────────────────────────────────

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

        // Kâr/Gas oranı
        let profit_margin_ratio = if gas_cost_weth > 0.00001 {
            expected_profit_weth / gas_cost_weth
        } else {
            10.0
        };

        // v24.0: Agresif PGA modülü — %10 ile %95 aralığında dinamik bribe.
        //
        // Base L2 sequencer sıralaması yalnızca priority fee ile belirlenir.
        // Rekabetçi bloklarda rakip botlar kârın %99'una kadar rüşvet
        // teklif edebilir. Düşük marjlı fırsatlarda agresif olmak gerekir.
        //
        // Yeni kademeler:
        //   margin >= 10x → %10 (çok düşük rekabet, kârı koru)
        //   margin 5-10x  → %25 (düşük rekabet)
        //   margin 3-5x   → %40 (orta rekabet)
        //   margin 2-3x   → %60 (yüksek rekabet)
        //   margin 1.5-2x → %80 (çok yüksek rekabet)
        //   margin < 1.5x → %95 (maksimum agresiflik — rakipleri ez)
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
            0.95 // v24.0: Eski %70 → %95 (maksimum rekabet gücü)
        };

        // v20.0: Minimum mutlak kâr koruması
        // Bribe sonrası kalan kâr en az 0.0001 WETH (~$0.25) olmalı.
        // Bu, L1 Data Fee dalgalanmasını karşılayacak statik güvenlik marjıdır.
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
    /// Toplam bribe miktarı (wei)
    pub bribe_wei: u128,
    /// Gas başına priority fee (wei)
    pub priority_fee_per_gas: u128,
    /// Uygulanan efektif bribe yüzdesi
    pub effective_pct: f64,
    /// Kâr/gas marj oranı
    pub profit_margin_ratio: f64,
    /// Gas maliyeti (WETH)
    pub gas_cost_weth: f64,
}

/// Yeni keşfedilen havuzları on-chain whiteliste eklemek için ABI-encoded calldata oluştur.
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
