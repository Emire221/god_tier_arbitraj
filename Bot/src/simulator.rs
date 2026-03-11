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

use crate::types::{DexType, PoolConfig, SharedPoolState, SimulationResult};
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
// v20.0: DEX-Spesifik Storage Layout Şablonları
// ─────────────────────────────────────────────────────────────────────────────
//
// Her DEX'in akıllı kontrat mimarisi farklı storage layout kullanır.
// Bu yapı, her DEX için bağımsız okuma/yazma şablonu sağlar.
// Storage injection sırasında DEX türüne göre doğru ofsetler kullanılır.
//
// Farklar:
//   UniV3:     slot0 → slot 0 (7 alan, uint8 feeProtocol, 248 bit, 1 slot)
//              liquidity → slot 4
//              unlocked → slot 0, bit 240
//
//   PCS V3:    slot0 → slot 0 (7 alan, uint32 feeProtocol, 272 bit > 256, 2 slot!)
//              liquidity → slot 5
//              unlocked → slot 1, bit 32 (ayrı slot!)
//
//   Aerodrome: slot0 → slot 2 (6 alan, feeProtocol YOK, 240 bit, 1 slot)
//              liquidity → slot 5
//              unlocked → slot 2, bit 232
//              öncesinde: gauge (address) + nft+fee packed → slot 0-1
// ─────────────────────────────────────────────────────────────────────────────

/// DEX-spesifik storage layout tanımı
#[allow(dead_code)]
struct StorageLayout {
    /// slot0 storage slot indeksi
    slot0_index: RevmU256,
    /// liquidity storage slot indeksi
    liquidity_index: RevmU256,
    /// slot0'da unlocked flag'inin bit pozisyonu (None = ayrı slot'ta)
        unlocked_bit_in_slot0: Option<u32>,
    /// PCS V3 gibi unlocked ayrı slot'taysa: (slot_index, bit_pozisyonu)
    unlocked_separate_slot: Option<(RevmU256, u32)>,
}

impl StorageLayout {
    /// DEX türüne göre doğru storage layout'u döndür
    fn for_dex(dex: DexType) -> Self {
        match dex {
            DexType::UniswapV3 => StorageLayout {
                slot0_index: RevmU256::ZERO,
                liquidity_index: RevmU256::from(4),
                unlocked_bit_in_slot0: Some(240),
                unlocked_separate_slot: None,
            },
            DexType::PancakeSwapV3 => StorageLayout {
                slot0_index: RevmU256::ZERO,
                liquidity_index: RevmU256::from(5),
                unlocked_bit_in_slot0: None, // slot0'a SIĞMAZ (272 bit > 256)
                unlocked_separate_slot: Some((RevmU256::from(1), 32)), // slot 1, bit 32
            },
            DexType::Aerodrome => StorageLayout {
                slot0_index: RevmU256::from(2),
                liquidity_index: RevmU256::from(5),
                unlocked_bit_in_slot0: Some(232), // feeProtocol YOK, unlocked bit 232
                unlocked_separate_slot: None,
            },
        }
    }

    /// Bu layout'a göre slot0 ve ilişkili storage slot'larını DB'ye yaz
    fn inject_slot0(&self, db: &mut InMemoryDB, addr: RevmAddress, sqrt_price_x96: U256, tick: i32, dex: DexType) {
        let slot0_value = pack_slot0(sqrt_price_x96, tick, dex);
        let _ = db.insert_account_storage(addr, self.slot0_index, slot0_value);

        // PCS V3 gibi unlocked ayrı slot'taysa onu da yaz
        if let Some((slot_idx, _bit)) = &self.unlocked_separate_slot {
            let _ = db.insert_account_storage(addr, *slot_idx, pack_pcs_v3_slot1_unlocked());
        }
    }

    /// Bu layout'a göre liquidity storage slot'unu DB'ye yaz
    fn inject_liquidity(&self, db: &mut InMemoryDB, addr: RevmAddress, liquidity: u128) {
        let _ = db.insert_account_storage(addr, self.liquidity_index, RevmU256::from(liquidity));
    }
}

/// Uniswap V3 / PancakeSwap V3 / Aerodrome slot0 storage paketleme.
///
/// v17.0 KRİTİK DÜZELTME: DEX'e özel storage layout farkları.
///
/// UniV3 slot0 (7 alan, uint8 feeProtocol — 248 bit, TEK slot):
///   [bits 0..159]   sqrtPriceX96 (uint160)
///   [bits 160..183] tick (int24, two's complement)
///   [bits 184..199] observationIndex (uint16) — 0
///   [bits 200..215] observationCardinality (uint16) — 0
///   [bits 216..231] observationCardinalityNext (uint16) — 0
///   [bits 232..239] feeProtocol (uint8) — 0
///   [bits 240..247] unlocked (bool) — TRUE
///
/// PancakeSwap V3 slot0 (7 alan, uint32 feeProtocol — 272 bit > 256, İKİ slot!):
///   Storage Slot N:   sqrtPriceX96 + tick + observation alanları (232 bit)
///   Storage Slot N+1: feeProtocol (uint32, bit 0-31) + unlocked (bool, bit 32)
///   ÖNCEKİ BUG: unlocked slot N'de bit 240'a yazılıyordu → Pool Locked revert!
///
/// Aerodrome CLPool slot0 (6 alan, feeProtocol YOK — 240 bit, TEK slot):
///   [bits 0..159]   sqrtPriceX96 (uint160)
///   [bits 160..183] tick (int24)
///   [bits 184..231] observation alanları (48 bit)
///   [bits 232..239] unlocked (bool) — TRUE
fn pack_slot0(sqrt_price_x96: U256, tick: i32, dex: DexType) -> RevmU256 {
    let mut packed = U256::ZERO;

    // sqrtPriceX96 — lower 160 bits
    let mask_160 = (U256::from(1u64) << 160) - U256::from(1u64);
    packed = packed | (sqrt_price_x96 & mask_160);

    // tick — int24 at bit 160 (two's complement, masked to 24 bits)
    let tick_bits = if tick >= 0 {
        U256::from(tick as u32)
    } else {
        // Two's complement for negative: 0xFFFFFF & tick
        let abs_val = (-tick) as u32;
        let twos = 0x01_000_000u32.wrapping_sub(abs_val);
        U256::from(twos)
    };
    let mask_24 = U256::from(0x00FF_FFFFu32);
    packed = packed | ((tick_bits & mask_24) << 160);

    // unlocked = true (1) — position depends on DEX type
    // PCS V3: uint32 feeProtocol (32 bit) slot 0'a SIĞMAZ (232+32=264 > 256)
    //         feeProtocol + unlocked → slot N+1'e taşar
    //         Bu yüzden slot 0'a unlocked yazılMAZ
    match dex {
        DexType::PancakeSwapV3 => {
            // PCS V3: unlocked slot 0'da değil, slot N+1'de (bit 32)
            // Burada sadece sqrtPriceX96 + tick yazılır
        }
        DexType::Aerodrome => {
            // Aerodrome: feeProtocol yok, unlocked bit 232
            packed = packed | (U256::from(1u64) << 232);
        }
        _ => {
            // UniV3 / SushiSwap V3: uint8 feeProtocol, unlocked bit 240
            packed = packed | (U256::from(1u64) << 240);
        }
    }

    to_revm_u256(packed)
}

/// PancakeSwap V3 slot0 ikinci storage slot'u (slot N+1).
///
/// PCS V3 Slot0 struct'ında uint32 feeProtocol 256 bit sınırını aştığı için
/// feeProtocol + unlocked ayrı bir slot'a taşar:
///   [bits 0..31]  feeProtocol (uint32) — 0 yazılır
///   [bits 32..39] unlocked (bool) — TRUE olmalı
fn pack_pcs_v3_slot1_unlocked() -> RevmU256 {
    to_revm_u256(U256::from(1u64) << 32)
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
    /// v22.1: Arbitraj kontrat bytecode'u (zincirden alınmış)
    /// build_db'de kontrat hesabına yüklenir — simülasyon gerçekçi olur
    contract_bytecode: Option<Vec<u8>>,
    /// v22.1: Zincir ID'si (config'den alınır, hardcoded değil)
    chain_id: u64,
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
            contract_bytecode: None,
            chain_id: 8453, // Varsayılan: Base
            base_db: None,
            base_caller: None,
            base_contract: None,
        }
    }

    /// v22.1: Zincir ID'sini ayarla (config'den)
    pub fn set_chain_id(&mut self, chain_id: u64) {
        self.chain_id = chain_id;
    }

    /// v22.1: Kontrat bytecode'unu ayarla (zincirden alınmış)
    pub fn set_contract_bytecode(&mut self, bytecode: Vec<u8>) {
        self.contract_bytecode = Some(bytecode);
    }

    /// Havuz bytecode'larını önbelleğe al
    ///
    /// v25.0: Append-only mode — mevcut cache temizlenmez, yeni havuzlar eklenir.
    /// Hot-reload sırasında sadece yeni havuzlar (slice) ile çağrılabilir;
    /// clear() eski havuzların bytecode'larını siliyordu.
    pub fn cache_bytecodes(&mut self, pools: &[PoolConfig], states: &[SharedPoolState]) {
        for (config, state_lock) in pools.iter().zip(states.iter()) {
            // Mevcut adres zaten cache'te varsa atla
            if self.bytecode_cache.iter().any(|(addr, _)| *addr == config.address) {
                continue;
            }
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

        // v20.0: StorageLayout şablonu ile DEX-bağımsız storage injection
        for (config, state_lock) in pools.iter().zip(states.iter()) {
            let state = state_lock.read();
            let addr = to_revm_addr(config.address);
            let layout = StorageLayout::for_dex(config.dex);

            layout.inject_slot0(&mut db, addr, state.sqrt_price_x96, state.tick, config.dex);
            layout.inject_liquidity(&mut db, addr, state.liquidity);
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

            // v20.0: StorageLayout şablonu ile DEX-bağımsız storage injection
            let layout = StorageLayout::for_dex(config.dex);
            layout.inject_slot0(&mut db, addr, state.sqrt_price_x96, state.tick, config.dex);
            layout.inject_liquidity(&mut db, addr, state.liquidity);
        }

        // ── Caller Hesabı (Test ETH Bakiyesi) ─────────────────────
        db.insert_account_info(
            to_revm_addr(caller),
            AccountInfo::from_balance(RevmU256::from(100_000_000_000_000_000_000u128)), // 100 ETH
        );

        // ── Kontrat Hesabı ──────────────────────────────────────────
        // v22.1: Kontrat bytecode'u varsa yükle — simülasyon gerçekçi olur.
        // Bytecode yoksa boş hesap (sadece gas tahmini olarak kullanılır).
        if let Some(ref code) = self.contract_bytecode {
            let bytecode = Bytecode::new_raw(RevmBytes::from(code.clone()));
            let info = AccountInfo::new(
                RevmU256::ZERO,
                0,
                bytecode.hash_slow(),
                bytecode,
            );
            db.insert_account_info(to_revm_addr(contract), info);
        } else {
            let contract_info = AccountInfo::from_balance(RevmU256::ZERO);
            db.insert_account_info(to_revm_addr(contract), contract_info);
        }

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
                cfg.chain_id = self.chain_id; // v22.1: config'den, hardcoded değil
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
    ///   - Gerçek token kapasitesi (Δx/Δy) yeterli mi?
    ///   - Fiyat makul aralıkta mı?
    ///
    /// v20.0 KRİTİK DÜZELTME: Likidite kontrolü artık L parametresi ile
    /// doğrudan karşılaştırma YAPMAZ. L, token miktarı değil, fiyat eğrisi
    /// oranıdır. Bunun yerine, SqrtPriceMath formülleri (Δx, Δy) ile
    /// havuzun mevcut fiyatından hedef fiyata kadar absorbe edebileceği
    /// gerçek WETH miktarı hesaplanır.
    ///
    /// Bu fonksiyon REVM'in eksik state nedeniyle hatalı sonuç vereceği
    /// durumlar için fallback olarak kullanılır.
    pub fn validate_mathematical(
        &self,
        pools: &[PoolConfig],
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

        // 2. Fiyatlar makul aralıkta mı? (Anormal fiyat → önce kontrol et)
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

        // 3. Veri taze mi?
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

        // 4. v20.0: Gerçek token kapasitesi kontrolü (V3 fiyat eğrisi matematiği)
        //    ESKİ (HATALI): amount_weth * 1e18 * 10.0 vs liquidity_f64
        //      → L bir token miktarı DEĞİLDİR, bu karşılaştırma her zaman
        //        geçerli işlemleri "Yetersiz Likidite" olarak reddediyordu.
        //    YENİ: hard_liquidity_cap_weth() ile mevcut sqrtPriceX96'dan
        //          itibaren V3 SqrtPriceMath formülleri (Δx = L·Q96·(1/√P_target - 1/√P)
        //          veya Δy = L·(√P_target - √P)/Q96) kullanılarak havuzun
        //          gerçek absorbe edebileceği WETH miktarı hesaplanır.
        {
            let buy_pool = &pools[buy_pool_idx];
            let sell_pool = &pools[sell_pool_idx];

            let buy_cap = math::exact::hard_liquidity_cap_weth(
                buy_state.sqrt_price_x96,
                buy_state.liquidity,
                buy_state.tick,
                buy_pool.token0_is_weth,
                buy_state.tick_bitmap.as_ref(),
                buy_pool.tick_spacing,
            );
            let sell_cap = math::exact::hard_liquidity_cap_weth(
                sell_state.sqrt_price_x96,
                sell_state.liquidity,
                sell_state.tick,
                sell_pool.token0_is_weth,
                sell_state.tick_bitmap.as_ref(),
                sell_pool.tick_spacing,
            );

            let effective_cap = buy_cap.min(sell_cap);

            if effective_cap < amount_weth {
                return SimulationResult {
                    success: false,
                    gas_used: 0,
                    error: Some(format!(
                        "Yetersiz V3 likidite kapasitesi: AL_cap={:.4} SAT_cap={:.4} WETH, \u{0130}stenen={:.4} WETH",
                        buy_cap, sell_cap, amount_weth
                    )),
                };
            }
        }

        // Tüm kontroller geçti
        // v20.0: Dinamik gas tahmini — swap adımı sayısına göre.
        // Tipik V3 single-pool swap: ~130K gas
        // Çapraz swap (2 havuz): ~260K gas
        // Tick geçişi başına ~+20K gas ek yük
        // Kontrat overhead (flash loan + callback): ~50K gas
        // Toplam tahmini: 260K + 50K = ~310K (minimum taban)
        let estimated_gas: u64 = {
            let base_gas: u64 = 310_000; // 2-havuz çapraz swap baz gas
            // TickBitmap varsa tahmini tick geçişi ekle
            let buy_tick_crossings = buy_state.tick_bitmap.as_ref()
                .map(|bm| bm.initialized_tick_count().min(5) as u64)
                .unwrap_or(1);
            let sell_tick_crossings = sell_state.tick_bitmap.as_ref()
                .map(|bm| bm.initialized_tick_count().min(5) as u64)
                .unwrap_or(1);
            let tick_cross_gas = (buy_tick_crossings + sell_tick_crossings) * 20_000;
            base_gas + tick_cross_gas
        };

        SimulationResult {
            success: true,
            gas_used: estimated_gas,
            error: None,
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

// ─────────────────────────────────────────────────────────────────────────────
// L2 Sequencer Reorg & Stale State Testleri
// ─────────────────────────────────────────────────────────────────────────────
//
// Base, tekil sequencer kullanan bir L2'dir. Sequencer yoğunluk anlarında
// reorg yapabilir — iyimser (optimistic) state güncellemesi yapılmışsa
// bot "hayalet fırsat" üzerine işlem gönderebilir. Bu test modülü,
// validate_mathematical fonksiyonunun bayat (stale) veya tutarsız state'leri
// doğru şekilde reddettiğini kanıtlar.
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod sequencer_reorg_tests {
    use super::*;
    use alloy::primitives::{Address, U256, address};
    use std::sync::Arc;
    use parking_lot::RwLock;
    use std::time::{Instant, Duration};
    use crate::types::*;

    const POOL_A: Address = address!("d0b53D9277642d899DF5C87A3966A349A798F224");
    const POOL_B: Address = address!("cDAC0d6c6C59727a65F871236188350531885C43");

    fn make_pool_configs() -> Vec<PoolConfig> {
        vec![
            PoolConfig {
                address: POOL_A,
                name: "UniV3-test".into(),
                fee_bps: 5,
                fee_fraction: 0.0005,
                token0_decimals: 18,
                token1_decimals: 6,
                dex: DexType::UniswapV3,
                token0_is_weth: true,
                tick_spacing: 10,
                quote_token_address: address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"),
            },
            PoolConfig {
                address: POOL_B,
                name: "Aero-test".into(),
                fee_bps: 100,
                fee_fraction: 0.01,
                token0_decimals: 18,
                token1_decimals: 6,
                dex: DexType::Aerodrome,
                token0_is_weth: true,
                tick_spacing: 1,
                quote_token_address: address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"),
            },
        ]
    }

    fn make_active_state(eth_price: f64, liquidity: u128, block: u64) -> SharedPoolState {
        // v20.0: Gerçekçi sqrtPriceX96 — WETH/USDC (18dec/6dec) formatında
        let price_ratio = eth_price * 1e-12;
        let tick = (price_ratio.ln() / 1.0001_f64.ln()).floor() as i32;
        let sqrt_price_x96_u256 = crate::math::exact::get_sqrt_ratio_at_tick(tick);
        let q96_f64: f64 = 2.0_f64.powi(96);
        let sqrt_price_f64 = price_ratio.sqrt() * q96_f64;

        Arc::new(RwLock::new(PoolState {
            sqrt_price_x96: sqrt_price_x96_u256,
            sqrt_price_f64: sqrt_price_f64,
            tick,
            liquidity,
            liquidity_f64: liquidity as f64,
            eth_price_usd: eth_price,
            last_block: block,
            last_update: Instant::now(),
            is_initialized: true,
            bytecode: None,
            tick_bitmap: None,
            live_fee_bps: None,
        }))
    }

    /// L2 Sequencer Reorg Testi: Bayat state reddedilmeli.
    ///
    /// Senaryo: Bot pending TX'den iyimser state güncellemesi yaptı.
    /// Sequencer bu TX'i düşürdü (dropped). State artık bayat.
    /// validate_mathematical() 5000ms staleness eşiğini aşan veriyi reddetmeli.
    #[test]
    fn test_sequencer_reorg_handling() {
        let pools = make_pool_configs();
        let sim = SimulationEngine::new();

        // Havuz A: taze state (henüz güncel)
        let state_a = make_active_state(2500.0, 10_000_000_000_000_000_000, 100);

        // Havuz B: bayat state (pending TX düşürüldü → state güncellenmedi)
        // Son güncelleme 6 saniye önceydi → staleness_ms() > 5000
        let price_ratio_b: f64 = 2510.0 * 1e-12;
        let tick_b = (price_ratio_b.ln() / 1.0001_f64.ln()).floor() as i32;
        let sqrt_price_x96_b = crate::math::exact::get_sqrt_ratio_at_tick(tick_b);
        let q96_f = 2.0_f64.powi(96);
        let state_b = Arc::new(RwLock::new(PoolState {
            sqrt_price_x96: sqrt_price_x96_b,
            sqrt_price_f64: price_ratio_b.sqrt() * q96_f,
            tick: tick_b,
            liquidity: 10_000_000_000_000_000_000,
            liquidity_f64: 10_000_000_000_000_000_000.0,
            eth_price_usd: 2510.0,
            last_block: 98, // 2 blok geride — reorg sonrası
            last_update: Instant::now() - Duration::from_secs(6), // 6s bayat
            is_initialized: true,
            bytecode: None,
            tick_bitmap: None,
            live_fee_bps: None,
        }));

        let states: Vec<SharedPoolState> = vec![state_a, state_b];

        // Simülasyon: bayat state'li havuz → BAŞARISIZ olmalı
        let result = sim.validate_mathematical(&pools, &states, 0, 1, 1.0);
        assert!(!result.success, "Bayat (stale) state ile simülasyon reddedilmeli");
        assert!(
            result.error.as_deref().unwrap_or("").contains("Bayat"),
            "Hata mesajı 'Bayat' içermeli, aldığımız: {:?}",
            result.error
        );
    }

    /// "Hayalet fırsat" testleri: Sequencer reorg sonrası geçersiz fiyatlar.
    ///
    /// Senaryo: Pending TX'den alınan fiyat 2500$, ancak TX düşürüldüğünde
    /// gerçek fiyat 0$ (veri yok/sıfırlandı). validate_mathematical bunu
    /// "Havuz aktif değil" olarak reddetmeli.
    #[test]
    fn test_sequencer_reorg_phantom_opportunity() {
        let pools = make_pool_configs();
        let sim = SimulationEngine::new();

        // İyimser state: pending TX'den alınan fiyat ($2500)
        let state_a = make_active_state(2500.0, 10_000_000_000_000_000_000, 100);

        // Reorg sonrası havuz B: fiyat sıfırlandı (dropped TX)
        let state_b = Arc::new(RwLock::new(PoolState {
            sqrt_price_x96: U256::ZERO,
            sqrt_price_f64: 0.0,
            tick: 0,
            liquidity: 0, // Likidite de sıfır
            liquidity_f64: 0.0,
            eth_price_usd: 0.0,
            last_block: 100,
            last_update: Instant::now(),
            is_initialized: false, // Havuz başlatılmamış gibi
            bytecode: None,
            tick_bitmap: None,
            live_fee_bps: None,
        }));

        let states: Vec<SharedPoolState> = vec![state_a, state_b];

        let result = sim.validate_mathematical(&pools, &states, 0, 1, 1.0);
        assert!(!result.success, "Hayalet fırsat (phantom opportunity) reddedilmeli");
        assert!(
            result.error.as_deref().unwrap_or("").contains("aktif değil"),
            "Hata mesajı 'aktif değil' içermeli: {:?}",
            result.error
        );
    }

    /// Çift bayat state testi: Her iki havuz da stale.
    ///
    /// Senaryo: Sequencer tam kesintide, hiçbir güncelleme gelmiyor.
    /// Tüm havuzlar 10+ saniye bayat → simülasyon kesinlikle reddedilmeli.
    #[test]
    fn test_sequencer_full_outage_both_pools_stale() {
        let pools = make_pool_configs();
        let sim = SimulationEngine::new();

        let stale_state = |price: f64| -> SharedPoolState {
            let pr = price * 1e-12;
            let t = (pr.ln() / 1.0001_f64.ln()).floor() as i32;
            let sqpx96 = crate::math::exact::get_sqrt_ratio_at_tick(t);
            let qf = 2.0_f64.powi(96);
            Arc::new(RwLock::new(PoolState {
                sqrt_price_x96: sqpx96,
                sqrt_price_f64: pr.sqrt() * qf,
                tick: t,
                liquidity: 10_000_000_000_000_000_000,
                liquidity_f64: 10_000_000_000_000_000_000.0,
                eth_price_usd: price,
                last_block: 95,
                last_update: Instant::now() - Duration::from_secs(10), // 10s bayat
                is_initialized: true,
                bytecode: None,
                tick_bitmap: None,
                live_fee_bps: None,
            }))
        };

        let states: Vec<SharedPoolState> = vec![stale_state(2500.0), stale_state(2520.0)];

        let result = sim.validate_mathematical(&pools, &states, 0, 1, 0.5);
        assert!(!result.success, "Tam kesintide her iki bayat havuz reddedilmeli");
        assert!(
            result.error.as_deref().unwrap_or("").contains("Bayat"),
            "Hata 'Bayat' içermeli: {:?}",
            result.error
        );
    }

    /// Taze state → simülasyon başarılı olmalı (pozitif kontrol).
    #[test]
    fn test_fresh_state_passes_validation() {
        let pools = make_pool_configs();
        let sim = SimulationEngine::new();

        let states: Vec<SharedPoolState> = vec![
            make_active_state(2500.0, 10_000_000_000_000_000_000, 100),
            make_active_state(2520.0, 10_000_000_000_000_000_000, 100),
        ];

        let result = sim.validate_mathematical(&pools, &states, 0, 1, 1.0);
        assert!(result.success, "Taze state ile simülasyon başarılı olmalı");
        assert!(result.error.is_none(), "Hata mesajı olmamalı");
    }

    /// Anormal fiyat testi: Reorg sonrası havuz fiyatı saçma değere ulaşmış.
    #[test]
    fn test_sequencer_reorg_abnormal_price() {
        let pools = make_pool_configs();
        let sim = SimulationEngine::new();

        // Normal havuz
        let state_a = make_active_state(2500.0, 10_000_000_000_000_000_000, 100);
        // Reorg sonrası absürd fiyat — flash loan manipülasyonu veya veri bozulması
        let state_b = make_active_state(999_999.0, 10_000_000_000_000_000_000, 100);

        let states: Vec<SharedPoolState> = vec![state_a, state_b];

        let result = sim.validate_mathematical(&pools, &states, 0, 1, 1.0);
        // 999,999 < 100,000 sınırı aşılıyor → anormal fiyat reddedilmeli
        assert!(!result.success, "Anormal fiyat ($999,999) reddedilmeli");
        assert!(
            result.error.as_deref().unwrap_or("").contains("Anormal fiyat"),
            "Hata 'Anormal fiyat' içermeli: {:?}",
            result.error
        );
    }
}
