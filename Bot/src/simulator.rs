// ============================================================================
//  SIMULATOR v9.0 — REVM Tabanlı Yerel EVM Simülasyonu + Multi-Tick Engine
//
//  v9.0 Yenilikler:
//  ✓ 134-byte calldata (deadlineBlock: uint32 eklendi)
//  ✓ Kontrat v9.0 uyumu (executor/admin, deadline, kâr kontrat içinde)
//
//  v6.0 (korunuyor):
//  ✓ TickBitmap entegrasyonu — multi-tick swap impact analizi
//  ✓ Tick geçiş detayları (hangi tick'ler patlatıldı, likidite değişimi)
//  ✓ Gerçek bitmap yoksa otomatik dampening fallback
//
//  Mimari:
//    1. InMemoryDB (CacheDB<EmptyDB>) oluşturulur
//    2. Havuz bytecode ve kritik storage slot'ları önceden doldurulur
//    3. Arbitraj kontratı çağrısı yerel EVM'de çalıştırılır
//    4. Sonuç: Success → işlem gönder / Revert → işlemi atla
// ============================================================================

use alloy::primitives::{Address, U256};
use alloy::hex;

use revm::{
    Evm, InMemoryDB,
    primitives::{
        AccountInfo, Bytecode, ExecutionResult, TransactTo, SpecId,
        Address as RevmAddress, U256 as RevmU256, Bytes as RevmBytes,
    },
};

use crate::types::{PoolConfig, SharedPoolState, SimulationResult};
use crate::math;

// ─────────────────────────────────────────────────────────────────────────────
// Tip Dönüşüm Yardımcıları
// ─────────────────────────────────────────────────────────────────────────────

/// alloy Address → revm Address (aynı alloy-primitives, doğrudan dönüşüm)
fn to_revm_addr(addr: Address) -> RevmAddress {
    RevmAddress::from_slice(addr.as_slice())
}

/// alloy U256 → revm U256 (alanlar aynı — doğrudan dönüşüm)
fn to_revm_u256(val: U256) -> RevmU256 {
    let bytes = val.to_be_bytes::<32>();
    RevmU256::from_be_bytes(bytes)
}

// ─────────────────────────────────────────────────────────────────────────────
// Simülasyon Motoru
// ─────────────────────────────────────────────────────────────────────────────

/// Simülasyon motoru — havuz durumlarını REVM veritabanına yükler
///
/// v10.0 Singleton Mimarisi:
///   - base_db: Bot başlatıldığında bir kez oluşturulur (bytecode + hesaplar)
///   - Her blokta base_db klonlanır, sadece slot0/liquidity güncellenir
///   - Bytecode her döngüde yeniden yüklenmez → ~2-3ms tasarruf
pub struct SimulationEngine {
    /// Havuz bytecode önbellekleri (adres → bytecode)
    bytecode_cache: Vec<(Address, Vec<u8>)>,
    /// v10.0: Kalıcı temel veritabanı (bytecode + hesaplar yüklü)
    /// Her simulate() çağrısında klonlanır, sadece slot'lar güncellenir
    base_db: Option<InMemoryDB>,
    /// base_db'deki caller ve contract adresleri
    base_caller: Option<Address>,
    base_contract: Option<Address>,
}

impl SimulationEngine {
    /// Yeni SimulationEngine oluştur
    pub fn new() -> Self {
        Self {
            bytecode_cache: Vec::new(),
            base_db: None,
            base_caller: None,
            base_contract: None,
        }
    }

    /// Havuz bytecode'larını önbelleğe al
    pub fn cache_bytecodes(&mut self, pools: &[PoolConfig], states: &[SharedPoolState]) {
        self.bytecode_cache.clear();

        for (config, state_lock) in pools.iter().zip(states.iter()) {
            let state = state_lock.read();
            if let Some(ref code) = state.bytecode {
                self.bytecode_cache.push((config.address, code.clone()));
            }
        }
    }

    /// v10.0: Temel veritabanını bir kez oluştur (bytecode + hesaplar)
    ///
    /// Bot başlatıldığında cache_bytecodes() sonrası çağrılır.
    /// Bytecode ve hesap bilgileri kalıcı olarak base_db'ye yüklenir.
    /// Sonraki simulate() çağrılarında bu klonlanır — bytecode yeniden
    /// yüklenmez, sadece slot0 ve liquidity güncellenir.
    pub fn initialize_base_db(
        &mut self,
        pools: &[PoolConfig],
        states: &[SharedPoolState],
        caller: Address,
        contract: Address,
    ) {
        let db = self.build_db(pools, states, caller, contract);
        self.base_db = Some(db);
        self.base_caller = Some(caller);
        self.base_contract = Some(contract);
    }

    /// v10.0: base_db'yi klonla ve sadece değişen slot'ları güncelle
    ///
    /// Bytecode zaten base_db'de mevcut — yeniden yüklenmez.
    /// Sadece slot0 (sqrtPriceX96) ve slot4 (liquidity) güncellenir.
    /// Performans: ~0.05ms (eski: ~2-3ms)
    fn build_db_from_base(
        &self,
        pools: &[PoolConfig],
        states: &[SharedPoolState],
    ) -> InMemoryDB {
        let mut db = self.base_db.as_ref().unwrap().clone();

        // Sadece değişen storage slot'larını güncelle (bytecode DOKUNULMAZ)
        for (config, state_lock) in pools.iter().zip(states.iter()) {
            let state = state_lock.read();
            let addr = to_revm_addr(config.address);

            // Slot 0: slot0 (sqrtPriceX96, tick, vb. — packed)
            let slot0_value = to_revm_u256(state.sqrt_price_x96);
            let _ = db.insert_account_storage(addr, RevmU256::ZERO, slot0_value);

            // Slot 4: liquidity
            let liquidity_value = RevmU256::from(state.liquidity);
            let _ = db.insert_account_storage(addr, RevmU256::from(4), liquidity_value);
        }

        db
    }

    /// InMemoryDB oluştur ve havuz durumlarını doldur
    fn build_db(
        &self,
        pools: &[PoolConfig],
        states: &[SharedPoolState],
        caller: Address,
        contract: Address,
    ) -> InMemoryDB {
        let mut db = InMemoryDB::default();

        // ── Havuz Kontratlarını Yükle ──────────────────────────────
        for (config, state_lock) in pools.iter().zip(states.iter()) {
            let state = state_lock.read();
            let addr = to_revm_addr(config.address);

            // Bytecode
            if let Some(ref code) = state.bytecode {
                let bytecode = Bytecode::new_raw(RevmBytes::from(code.clone()));
                let info = AccountInfo::new(
                    RevmU256::ZERO,
                    0,
                    bytecode.hash_slow(),
                    bytecode,
                );
                db.insert_account_info(addr, info);
            }

            // Kritik storage slot'ları
            // Slot 0: slot0 (sqrtPriceX96, tick, vb. — packed)
            let slot0_value = to_revm_u256(state.sqrt_price_x96);
            let _ = db.insert_account_storage(addr, RevmU256::ZERO, slot0_value);

            // Slot 4: liquidity
            let liquidity_value = RevmU256::from(state.liquidity);
            let _ = db.insert_account_storage(addr, RevmU256::from(4), liquidity_value);
        }

        // ── Caller Hesabı (Test ETH Bakiyesi) ─────────────────────
        db.insert_account_info(
            to_revm_addr(caller),
            AccountInfo::from_balance(RevmU256::from(100_000_000_000_000_000_000u128)), // 100 ETH
        );

        // ── Kontrat Hesabı (Eğer bytecode varsa) ─────────────────
        // NOT: Gerçek kontrat bytecode'u zincirden alınmalıdır.
        // Şimdilik boş hesap oluşturulur — kontrat yoksa simülasyon
        // sadece gas tahmini olarak kullanılır.
        let contract_info = AccountInfo::from_balance(RevmU256::ZERO);
        db.insert_account_info(to_revm_addr(contract), contract_info);

        db
    }

    /// Arbitraj işlemini REVM'de simüle et
    ///
    /// Simülasyon adımları:
    ///   1. InMemoryDB'yi güncel havuz verileriyle doldur
    ///   2. EVM ortamını yapılandır (caller, hedef, calldata, gas)
    ///   3. İşlemi yerel olarak çalıştır
    ///   4. Sonucu analiz et (Success/Revert/Halt)
    ///
    /// # Notlar
    /// - Dış RPC çağrısı YAPILMAZ — tamamen yerel
    /// - İlk block için ~0.5ms, sonraki bloklar için <0.1ms
    pub fn simulate(
        &self,
        pools: &[PoolConfig],
        states: &[SharedPoolState],
        caller: Address,
        contract_address: Address,
        calldata: Vec<u8>,
        value_wei: U256,
        current_block: u64,
        block_timestamp: u64,
        block_base_fee: u64,
    ) -> SimulationResult {
        // 1. Veritabanını oluştur
        // v10.0: base_db varsa klonla+güncelle (hızlı), yoksa sıfırdan oluştur (fallback)
        let db = if self.base_db.is_some() {
            self.build_db_from_base(pools, states)
        } else {
            self.build_db(pools, states, caller, contract_address)
        };

        // 2. EVM'yi yapılandır ve çalıştır
        // v10.0: Timestamp ve base_fee artık zincir verisinden dinamik olarak gelir.
        //        Eski: SystemTime::now() → yanlış zaman damgası, base_fee yok
        //        Yeni: block_header.timestamp ve block_header.base_fee_per_gas
        let mut evm = Evm::builder()
            .with_db(db)
            .with_spec_id(SpecId::CANCUN)
            .modify_cfg_env(|cfg| {
                cfg.chain_id = 8453; // Base
            })
            .modify_block_env(|block| {
                block.number = RevmU256::from(current_block);
                block.timestamp = RevmU256::from(block_timestamp);
                block.basefee = RevmU256::from(block_base_fee);
            })
            .modify_tx_env(|tx| {
                tx.caller = to_revm_addr(caller);
                tx.transact_to = TransactTo::Call(to_revm_addr(contract_address));
                tx.data = RevmBytes::from(calldata);
                tx.value = to_revm_u256(value_wei);
                tx.gas_limit = 1_500_000;
                tx.nonce = None; // Nonce kontrolünü atla
            })
            .build();

        // 3. İşlemi çalıştır
        match evm.transact() {
            Ok(result_and_state) => {
                match result_and_state.result {
                    ExecutionResult::Success { gas_used, .. } => {
                        SimulationResult {
                            success: true,
                            gas_used,
                            error: None,
                        }
                    }
                    ExecutionResult::Revert { gas_used, output } => {
                        SimulationResult {
                            success: false,
                            gas_used,
                            error: Some(format!(
                                "REVERT: 0x{}",
                                output.iter().map(|b| format!("{:02x}", b)).collect::<String>()
                            )),
                        }
                    }
                    ExecutionResult::Halt { reason, gas_used } => {
                        SimulationResult {
                            success: false,
                            gas_used,
                            error: Some(format!("HALT: {:?}", reason)),
                        }
                    }
                }
            }
            Err(e) => {
                SimulationResult {
                    success: false,
                    gas_used: 0,
                    error: Some(format!("EVM hatası: {:?}", e)),
                }
            }
        }
    }

    /// Basit matematiksel doğrulama simülasyonu
    ///
    /// Tam REVM simülasyonu yerine hızlı bir kontrol yapar:
    ///   - Havuz verileri geçerli mi?
    ///   - Likidite yeterli mi?
    ///   - Fiyat makul aralıkta mı?
    ///
    /// Bu fonksiyon REVM'in eksik state nedeniyle hatalı sonuç vereceği
    /// durumlar için fallback olarak kullanılır.
    pub fn validate_mathematical(
        &self,
        _pools: &[PoolConfig],
        states: &[SharedPoolState],
        buy_pool_idx: usize,
        sell_pool_idx: usize,
        amount_weth: f64,
    ) -> SimulationResult {
        // Temel doğrulamalar
        let buy_state = states[buy_pool_idx].read();
        let sell_state = states[sell_pool_idx].read();

        // 1. Havuzlar aktif mi?
        if !buy_state.is_active() || !sell_state.is_active() {
            return SimulationResult {
                success: false,
                gas_used: 0,
                error: Some("Havuz(lar) aktif değil".into()),
            };
        }

        // 2. Likidite yeterli mi? (işlem boyutu likiditenin %10'unu aşmasın)
        let min_liquidity = amount_weth * 1e18 * 10.0; // Minimum 10x likidite
        if buy_state.liquidity_f64 < min_liquidity || sell_state.liquidity_f64 < min_liquidity {
            return SimulationResult {
                success: false,
                gas_used: 0,
                error: Some(format!(
                    "Yetersiz likidite: AL={:.0}, SAT={:.0}, Minimum={:.0}",
                    buy_state.liquidity_f64, sell_state.liquidity_f64, min_liquidity
                )),
            };
        }

        // 3. Fiyatlar makul aralıkta mı?
        if buy_state.eth_price_usd < 100.0
            || buy_state.eth_price_usd > 100_000.0
            || sell_state.eth_price_usd < 100.0
            || sell_state.eth_price_usd > 100_000.0
        {
            return SimulationResult {
                success: false,
                gas_used: 0,
                error: Some(format!(
                    "Anormal fiyat: AL={:.2}, SAT={:.2}",
                    buy_state.eth_price_usd, sell_state.eth_price_usd
                )),
            };
        }

        // 4. Veri taze mi?
        if buy_state.staleness_ms() > 5000 || sell_state.staleness_ms() > 5000 {
            return SimulationResult {
                success: false,
                gas_used: 0,
                error: Some(format!(
                    "Bayat veri: AL={}ms, SAT={}ms",
                    buy_state.staleness_ms(), sell_state.staleness_ms()
                )),
            };
        }

        // Tüm kontroller geçti
        SimulationResult {
            success: true,
            gas_used: 350_000, // Fallback tahmini gas — tam REVM simülasyonu varsa o değer kullanılır
            error: None,
        }
    }

    /// REVM + Multi-Tick tabanlı swap impact simülasyonu.
    ///
    /// TickBitmap varsa gerçek tick geçişlerini modelleyerek:
    ///   - "50 ETH satarsam hangi tick'leri patlatırım?"
    ///   - "Ortalama fiyatım ne olur?"
    ///   - "Toplam slippage ne kadar?"
    /// sorularına mikrosaniye içinde cevap verir.
    #[allow(dead_code)]
    pub fn estimate_swap_impact(
        &self,
        pools: &[PoolConfig],
        states: &[SharedPoolState],
        pool_idx: usize,
        amount_weth: f64,
    ) -> SwapImpactResult {
        if pool_idx >= pools.len() || pool_idx >= states.len() {
            return SwapImpactResult::failed("Geçersiz havuz indeksi");
        }

        let config = &pools[pool_idx];
        let state = states[pool_idx].read();

        if !state.is_active() {
            return SwapImpactResult::failed("Havuz aktif değil");
        }

        // ── 1. REVM ile state doğrulama (opsiyonel) ─────────────────
        let revm_validated = if state.bytecode.is_some() {
            self.validate_state_via_revm(pools, states, pool_idx)
        } else {
            true
        };

        // ── 2. Multi-Tick Swap Impact Hesabı (TickBitmap destekli) ───
        let current_tick = state.tick;
        let sqrt_price_f64 = state.sqrt_price_f64;
        let liquidity = state.liquidity_f64;

        // Güvenli maksimum swap miktarı
        let max_safe = math::max_safe_swap_amount(
            sqrt_price_f64, liquidity, config.token0_is_weth,
        );

        // TickBitmap referansı al
        let bitmap_ref = state.tick_bitmap.as_ref();

        // Multi-tick swap (TickBitmap varsa gerçek, yoksa dampening)
        let swap_result = math::swap_weth_to_usdc_multitick(
            sqrt_price_f64,
            liquidity,
            current_tick,
            amount_weth,
            config.fee_fraction,
            config.token0_is_weth,
            config.tick_spacing,
            bitmap_ref,
        );

        let usdc_output = swap_result.total_output;
        let effective_price = swap_result.effective_price;

        // Slippage hesabı
        let slippage_pct = if state.eth_price_usd > 0.0 {
            ((state.eth_price_usd - effective_price) / state.eth_price_usd).abs() * 100.0
        } else {
            0.0
        };

        // Tick geçiş detayları
        let tick_crossings_count = swap_result.tick_crossings.len() as u32;
        let tick_crossings_detail: Vec<(i32, f64, i128)> = swap_result.tick_crossings.iter()
            .map(|c| (c.tick, c.output_produced, c.liquidity_net))
            .collect();

        SwapImpactResult {
            success: true,
            usdc_output,
            effective_price,
            current_tick,
            final_tick: swap_result.final_tick,
            slippage_pct,
            max_safe_amount: max_safe,
            revm_validated,
            used_real_bitmap: swap_result.used_real_bitmap,
            tick_crossings_count,
            tick_crossings_detail,
            error: None,
        }
    }

    /// REVM üzerinden havuz state'ini doğrula.
    /// InMemoryDB'ye yüklenen slot0 verisinin tutarlılığını kontrol eder.
    #[allow(dead_code)]
    fn validate_state_via_revm(
        &self,
        pools: &[PoolConfig],
        states: &[SharedPoolState],
        pool_idx: usize,
    ) -> bool {
        let config = &pools[pool_idx];
        let state = states[pool_idx].read();
        let addr = to_revm_addr(config.address);

        // Basit doğrulama: slot0 storage değeri RAM ile tutarlı mı?
        let stored_sqrt = to_revm_u256(state.sqrt_price_x96);
        let db_sqrt = {
            let db = self.build_db(
                pools, states,
                Address::ZERO, Address::ZERO,
            );
            db.accounts.get(&addr)
                .and_then(|acc| acc.storage.get(&RevmU256::ZERO))
                .copied()
                .unwrap_or(RevmU256::ZERO)
        };

        // sqrtPriceX96 slot0'ın alt 160 bit'inde saklanır
        // Basit tutarlılık kontrolü: sıfır değilse geçerli
        db_sqrt != RevmU256::ZERO && stored_sqrt != RevmU256::ZERO
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Swap Impact Sonucu
// ─────────────────────────────────────────────────────────────────────────────

/// REVM + Multi-Tick tabanlı swap impact analizi sonucu
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SwapImpactResult {
    /// Simülasyon başarılı mı?
    pub success: bool,
    /// Tahmini USDC çıktısı
    pub usdc_output: f64,
    /// Efektif swap fiyatı (USDC/WETH)
    pub effective_price: f64,
    /// Mevcut tick (swap öncesi)
    pub current_tick: i32,
    /// Son tick (swap sonrası)
    pub final_tick: i32,
    /// Tahmini slippage yüzdesi
    pub slippage_pct: f64,
    /// Güvenli maksimum swap miktarı (WETH)
    pub max_safe_amount: f64,
    /// REVM ile doğrulandı mı?
    pub revm_validated: bool,
    /// Gerçek TickBitmap kullanıldı mı?
    pub used_real_bitmap: bool,
    /// Geçilen tick sınır sayısı
    pub tick_crossings_count: u32,
    /// Tick geçiş detayları: (tick, output, liquidityNet)
    pub tick_crossings_detail: Vec<(i32, f64, i128)>,
    /// Hata mesajı (varsa)
    pub error: Option<String>,
}

impl SwapImpactResult {
    #[allow(dead_code)]
    fn failed(msg: &str) -> Self {
        Self {
            success: false,
            usdc_output: 0.0,
            effective_price: 0.0,
            current_tick: 0,
            final_tick: 0,
            slippage_pct: 0.0,
            max_safe_amount: 0.0,
            revm_validated: false,
            used_real_bitmap: false,
            tick_crossings_count: 0,
            tick_crossings_detail: vec![],
            error: Some(msg.into()),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Calldata Payload Mühendisliği — 134 Byte Kompakt Kodlama (v9.0 Kontrat)
// ─────────────────────────────────────────────────────────────────────────────
//
// Kontrat v9.0 ile uyumlu 134-byte calldata formatı:
//
//   Offset  Boy   Alan
//   ──────  ────  ─────────────────────────────────
//   0x00    20B   Pool A adresi (UniV3 flash swap)
//   0x14    20B   Pool B adresi (Slipstream satış)
//   0x28    20B   owedToken (flash loan'a geri ödenen token adresi)
//   0x3C    20B   receivedToken (flash loan'dan alınan + slipstream'e ödenen)
//   0x50    32B   Miktar (uint256, big-endian)
//   0x70     1B   UniV3 Yön (0=zeroForOne, 1=oneForZero)
//   0x71     1B   Slipstream Yön (0=zeroForOne, 1=oneForZero)
//   0x72    16B   minProfit (uint128, big-endian — sandviç koruması)
//   0x82     4B   deadlineBlock (uint32, big-endian — blok son kullanma)
//   ──────  ────  ─────────────────────────────────
//   Toplam: 134B  (v8.0: 130B + 4B deadlineBlock)
//
// Gas tasarrufu: ABI'nin 260+ byte'ına karşı ~%48 tasarruf
// Güvenlik: minProfit + deadlineBlock ile MEV + stale TX koruması
// ─────────────────────────────────────────────────────────────────────────────

/// 134-byte kompakt calldata kodla (kontrat v9.0 uyumlu)
///
/// # Parametreler
/// - `pool_a`: UniV3 havuzu (flash swap kaynağı)
/// - `pool_b`: Slipstream havuzu (satış hedefi)
/// - `owed_token`: Flash loan geri ödemesi için token adresi
/// - `received_token`: Flash loan'dan alınan token adresi
/// - `amount_in_wei`: İşlem miktarı (uint256, big-endian)
/// - `uni_direction`: UniV3 yön (0=zeroForOne, 1=oneForZero)
/// - `aero_direction`: Slipstream yön (0=zeroForOne, 1=oneForZero)
/// - `min_profit`: Minimum kâr eşiği (uint128, wei cinsinden)
/// - `deadline_block`: Son geçerli blok numarası (uint32)
pub fn encode_compact_calldata(
    pool_a: Address,
    pool_b: Address,
    owed_token: Address,
    received_token: Address,
    amount_in_wei: U256,
    uni_direction: u8,
    aero_direction: u8,
    min_profit: u128,
    deadline_block: u32,
) -> Vec<u8> {
    // Tam 134 byte: 20+20+20+20+32+1+1+16+4
    let mut calldata = Vec::with_capacity(134);

    // [0x00..0x14] Pool A adresi (UniV3) — 20 byte
    calldata.extend_from_slice(pool_a.as_slice());

    // [0x14..0x28] Pool B adresi (Slipstream) — 20 byte
    calldata.extend_from_slice(pool_b.as_slice());

    // [0x28..0x3C] owedToken adresi — 20 byte
    calldata.extend_from_slice(owed_token.as_slice());

    // [0x3C..0x50] receivedToken adresi — 20 byte
    calldata.extend_from_slice(received_token.as_slice());

    // [0x50..0x70] Miktar — uint256, 32 byte big-endian
    calldata.extend_from_slice(&amount_in_wei.to_be_bytes::<32>());

    // [0x70] UniV3 Yön — 1 byte
    calldata.push(uni_direction);

    // [0x71] Slipstream Yön — 1 byte
    calldata.push(aero_direction);

    // [0x72..0x82] minProfit — uint128, 16 byte big-endian
    calldata.extend_from_slice(&min_profit.to_be_bytes());

    // [0x82..0x86] deadlineBlock — uint32, 4 byte big-endian
    calldata.extend_from_slice(&deadline_block.to_be_bytes());

    debug_assert_eq!(calldata.len(), 134, "Kompakt calldata tam 134 byte olmalı");
    calldata
}

/// Kompakt calldata'yı çözümle (test/debug için)
///
/// 134 byte → (pool_a, pool_b, owed_token, received_token, amount, uni_dir, aero_dir, min_profit, deadline_block)
#[allow(dead_code)]
pub fn decode_compact_calldata(data: &[u8]) -> Option<(Address, Address, Address, Address, U256, u8, u8, u128, u32)> {
    if data.len() != 134 {
        return None;
    }

    let pool_a = Address::from_slice(&data[0..20]);
    let pool_b = Address::from_slice(&data[20..40]);
    let owed_token = Address::from_slice(&data[40..60]);
    let received_token = Address::from_slice(&data[60..80]);
    let amount = U256::from_be_bytes::<32>(data[80..112].try_into().ok()?);
    let uni_direction = data[112];
    let aero_direction = data[113];
    let min_profit = u128::from_be_bytes(data[114..130].try_into().ok()?);
    let deadline_block = u32::from_be_bytes(data[130..134].try_into().ok()?);

    Some((pool_a, pool_b, owed_token, received_token, amount, uni_direction, aero_direction, min_profit, deadline_block))
}

/// Kompakt calldata'yı hex string olarak formatla (log/debug)
pub fn format_compact_calldata_hex(calldata: &[u8]) -> String {
    format!("0x{}", hex::encode(calldata))
}

// ─────────────────────────────────────────────────────────────────────────────
// Calldata Testleri (134-byte v9.0 formatı)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod calldata_tests {
    use super::*;
    use alloy::primitives::{U256, address};

    // Sabit test adresleri
    const POOL_A: Address = address!("d0b53D9277642d899DF5C87A3966A349A798F224");
    const POOL_B: Address = address!("cDAC0d6c6C59727a65F871236188350531885C43");
    const WETH: Address = address!("4200000000000000000000000000000000000006");
    const USDC: Address = address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913");

    #[test]
    fn test_compact_calldata_is_134_bytes() {
        let amount = U256::from(5_000_000_000_000_000_000u128); // 5 WETH

        let calldata = encode_compact_calldata(
            POOL_A, POOL_B, WETH, USDC,
            amount, 0x00, 0x01, 1_000_000u128, 99_999_999u32,
        );

        assert_eq!(calldata.len(), 134, "Kompakt calldata 134 byte olmalı");
    }

    #[test]
    fn test_compact_calldata_encode_decode_roundtrip() {
        let amount = U256::from(25_000_000_000_000_000_000u128); // 25 WETH
        let min_profit: u128 = 50_000_000; // 50 USDC (6 decimal)
        let deadline: u32 = 12_345_678;

        // Kodla
        let calldata = encode_compact_calldata(
            POOL_A, POOL_B, USDC, WETH,
            amount, 0x01, 0x00, min_profit, deadline,
        );
        assert_eq!(calldata.len(), 134);

        // Çözümle (round-trip)
        let (dec_a, dec_b, dec_owed, dec_recv, dec_amount, dec_uni, dec_aero, dec_profit, dec_deadline) =
            decode_compact_calldata(&calldata).expect("Decode başarısız");

        assert_eq!(dec_a, POOL_A, "Pool A adresi eşleşmeli");
        assert_eq!(dec_b, POOL_B, "Pool B adresi eşleşmeli");
        assert_eq!(dec_owed, USDC, "owedToken eşleşmeli");
        assert_eq!(dec_recv, WETH, "receivedToken eşleşmeli");
        assert_eq!(dec_amount, amount, "Miktar eşleşmeli");
        assert_eq!(dec_uni, 0x01, "UniV3 yön eşleşmeli");
        assert_eq!(dec_aero, 0x00, "Slipstream yön eşleşmeli");
        assert_eq!(dec_profit, min_profit, "minProfit eşleşmeli");
        assert_eq!(dec_deadline, deadline, "deadlineBlock eşleşmeli");
    }

    #[test]
    fn test_compact_calldata_byte_layout() {
        let pool_a = address!("0000000000000000000000000000000000000001");
        let pool_b = address!("0000000000000000000000000000000000000002");
        let owed = address!("0000000000000000000000000000000000000003");
        let recv = address!("0000000000000000000000000000000000000004");
        let amount = U256::from(1u64);
        let deadline: u32 = 0x01020304;

        let cd = encode_compact_calldata(pool_a, pool_b, owed, recv, amount, 0x00, 0x01, 0xFF, deadline);

        // [0..20] = pool_a → son byte 0x01
        assert_eq!(cd[19], 0x01, "Pool A son byte = 0x01");
        // [20..40] = pool_b → son byte 0x02
        assert_eq!(cd[39], 0x02, "Pool B son byte = 0x02");
        // [40..60] = owedToken → son byte 0x03
        assert_eq!(cd[59], 0x03, "owedToken son byte = 0x03");
        // [60..80] = receivedToken → son byte 0x04
        assert_eq!(cd[79], 0x04, "receivedToken son byte = 0x04");
        // [80..112] = amount (32 byte big-endian, değer=1, son byte=0x01)
        assert_eq!(cd[111], 0x01, "Amount son byte = 0x01");
        assert_eq!(cd[80], 0x00, "Amount ilk byte = 0x00");
        // [112] = UniV3 direction
        assert_eq!(cd[112], 0x00, "UniV3 dir = 0x00");
        // [113] = Aero direction
        assert_eq!(cd[113], 0x01, "Aero dir = 0x01");
        // [114..130] = minProfit (16 byte big-endian, 0xFF = last byte)
        assert_eq!(cd[129], 0xFF, "minProfit son byte = 0xFF");
        assert_eq!(cd[114], 0x00, "minProfit ilk byte = 0x00");
        // [130..134] = deadlineBlock (4 byte big-endian)
        assert_eq!(cd[130], 0x01, "deadlineBlock byte 0 = 0x01");
        assert_eq!(cd[131], 0x02, "deadlineBlock byte 1 = 0x02");
        assert_eq!(cd[132], 0x03, "deadlineBlock byte 2 = 0x03");
        assert_eq!(cd[133], 0x04, "deadlineBlock byte 3 = 0x04");
    }

    #[test]
    fn test_compact_calldata_invalid_length_rejected() {
        // 133 byte (eksik) — decode None döndürmeli
        let short = vec![0u8; 133];
        assert!(decode_compact_calldata(&short).is_none(), "133 byte reddedilmeli");

        // 135 byte (fazla) — decode None döndürmeli
        let long = vec![0u8; 135];
        assert!(decode_compact_calldata(&long).is_none(), "135 byte reddedilmeli");

        // Eski 130 byte — reddedilmeli
        let old = vec![0u8; 130];
        assert!(decode_compact_calldata(&old).is_none(), "130 byte eski format reddedilmeli");

        // Boş veri
        let empty: Vec<u8> = vec![];
        assert!(decode_compact_calldata(&empty).is_none(), "Boş veri reddedilmeli");
    }

    #[test]
    fn test_compact_vs_abi_size_comparison() {
        // Eski ABI: 4 (selector) + 32*9 (params) = 292 byte
        // Yeni kompakt: 20+20+20+20+32+1+1+16+4 = 134 byte
        // Tasarruf: 292 - 134 = 158 byte (~%54 azalma)
        let amount = U256::from(10_000_000_000_000_000_000u128);

        let compact = encode_compact_calldata(
            POOL_A, POOL_B, WETH, USDC,
            amount, 0x00, 0x01, 1_000_000u128, 99_999_999u32,
        );
        let abi_size: usize = 4 + 32 * 9; // 9 parametreli ABI

        assert_eq!(compact.len(), 134);
        assert_eq!(abi_size, 292);
        assert!(compact.len() < abi_size, "Kompakt format ABI'den küçük olmalı");

        let saved = abi_size - compact.len();
        assert_eq!(saved, 158, "158 byte tasarruf");
    }

    #[test]
    fn test_format_compact_calldata_hex() {
        let pool_a = address!("0000000000000000000000000000000000000001");
        let pool_b = address!("0000000000000000000000000000000000000002");
        let owed = address!("0000000000000000000000000000000000000003");
        let recv = address!("0000000000000000000000000000000000000004");
        let amount = U256::from(0xFFu64);

        let cd = encode_compact_calldata(pool_a, pool_b, owed, recv, amount, 0x00, 0x01, 0, 100u32);
        let hex_str = format_compact_calldata_hex(&cd);

        // "0x" ile başlamalı
        assert!(hex_str.starts_with("0x"), "Hex 0x ile başlamalı");
        // 134 byte = 268 hex karakter + "0x" = 270 karakter
        assert_eq!(hex_str.len(), 270, "Hex string 270 karakter olmalı");
    }

    #[test]
    fn test_min_profit_max_u128() {
        // u128::MAX minProfit testi
        let max_profit = u128::MAX;
        let calldata = encode_compact_calldata(
            POOL_A, POOL_B, WETH, USDC,
            U256::from(1u64), 0x00, 0x01, max_profit, u32::MAX,
        );
        assert_eq!(calldata.len(), 134);

        let (_, _, _, _, _, _, _, decoded_profit, decoded_deadline) =
            decode_compact_calldata(&calldata).expect("Decode başarısız");
        assert_eq!(decoded_profit, max_profit, "u128::MAX minProfit round-trip");
        assert_eq!(decoded_deadline, u32::MAX, "u32::MAX deadlineBlock round-trip");
    }

    #[test]
    fn test_real_base_scenario() {
        // Gerçek Base Network senaryosu:
        // UniV3 WETH/USDC 0.05% havuzundan flash swap ile USDC al
        // Slipstream'de USDC ile WETH geri al
        // owedToken = WETH (UniV3'e geri öde)
        // receivedToken = USDC (UniV3'den al, Slipstream'e ver)
        let amount = U256::from(1_000_000_000_000_000_000u128); // 1 WETH
        let min_profit = 500_000u128; // 0.5 USDC (6 decimal)
        let deadline = 20_000_000u32; // Blok ~20M

        let calldata = encode_compact_calldata(
            POOL_A, POOL_B,
            WETH,    // owedToken (WETH borçlu)
            USDC,    // receivedToken (USDC alınan)
            amount,
            0x01,   // UniV3: oneForZero (WETH al = zeroForOne=false → direction=1)
            0x00,   // Slipstream: zeroForOne (USDC sat → WETH al)
            min_profit,
            deadline,
        );

        assert_eq!(calldata.len(), 134);

        // Round-trip doğrula  
        let decoded = decode_compact_calldata(&calldata).expect("Decode başarısız");
        assert_eq!(decoded.0, POOL_A);
        assert_eq!(decoded.1, POOL_B);
        assert_eq!(decoded.2, WETH);
        assert_eq!(decoded.3, USDC);
        assert_eq!(decoded.4, amount);
        assert_eq!(decoded.5, 0x01);
        assert_eq!(decoded.6, 0x00);
        assert_eq!(decoded.7, min_profit);
        assert_eq!(decoded.8, deadline);
    }
}
