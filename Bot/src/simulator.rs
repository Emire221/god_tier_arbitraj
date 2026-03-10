// ============================================================================
//  SIMULATOR v9.0 â€” REVM TabanlÄ± Yerel EVM SimÃ¼lasyonu + Multi-Tick Engine
//
//  v9.0 Yenilikler:
//  âœ“ 134-byte calldata (deadlineBlock: uint32 eklendi)
//  âœ“ Kontrat v9.0 uyumu (executor/admin, deadline, kÃ¢r kontrat iÃ§inde)
//
//  v6.0 (korunuyor):
//  âœ“ TickBitmap entegrasyonu â€” multi-tick swap impact analizi
//  âœ“ Tick geÃ§iÅŸ detaylarÄ± (hangi tick'ler patlatÄ±ldÄ±, likidite deÄŸiÅŸimi)
//  âœ“ GerÃ§ek bitmap yoksa otomatik dampening fallback
//
//  Mimari:
//    1. InMemoryDB (CacheDB<EmptyDB>) oluÅŸturulur
//    2. Havuz bytecode ve kritik storage slot'larÄ± Ã¶nceden doldurulur
//    3. Arbitraj kontratÄ± Ã§aÄŸrÄ±sÄ± yerel EVM'de Ã§alÄ±ÅŸtÄ±rÄ±lÄ±r
//    4. SonuÃ§: Success â†’ iÅŸlem gÃ¶nder / Revert â†’ iÅŸlemi atla
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

// â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
// Tip DÃ¶nÃ¼ÅŸÃ¼m YardÄ±mcÄ±larÄ±
// â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

/// alloy Address â†’ revm Address (aynÄ± alloy-primitives, doÄŸrudan dÃ¶nÃ¼ÅŸÃ¼m)
fn to_revm_addr(addr: Address) -> RevmAddress {
    RevmAddress::from_slice(addr.as_slice())
}

/// alloy U256 â†’ revm U256 (alanlar aynÄ± â€” doÄŸrudan dÃ¶nÃ¼ÅŸÃ¼m)
fn to_revm_u256(val: U256) -> RevmU256 {
    let bytes = val.to_be_bytes::<32>();
    RevmU256::from_be_bytes(bytes)
}

// â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
// v20.0: DEX-Spesifik Storage Layout ÅablonlarÄ±
// â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
//
// Her DEX'in akÄ±llÄ± kontrat mimarisi farklÄ± storage layout kullanÄ±r.
// Bu yapÄ±, her DEX iÃ§in baÄŸÄ±msÄ±z okuma/yazma ÅŸablonu saÄŸlar.
// Storage injection sÄ±rasÄ±nda DEX tÃ¼rÃ¼ne gÃ¶re doÄŸru ofsetler kullanÄ±lÄ±r.
//
// Farklar:
//   UniV3:     slot0 â†’ slot 0 (7 alan, uint8 feeProtocol, 248 bit, 1 slot)
//              liquidity â†’ slot 4
//              unlocked â†’ slot 0, bit 240
//
//   PCS V3:    slot0 â†’ slot 0 (7 alan, uint32 feeProtocol, 272 bit > 256, 2 slot!)
//              liquidity â†’ slot 5
//              unlocked â†’ slot 1, bit 32 (ayrÄ± slot!)
//
//   Aerodrome: slot0 â†’ slot 2 (6 alan, feeProtocol YOK, 240 bit, 1 slot)
//              liquidity â†’ slot 5
//              unlocked â†’ slot 2, bit 232
//              Ã¶ncesinde: gauge (address) + nft+fee packed â†’ slot 0-1
// â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

/// DEX-spesifik storage layout tanÄ±mÄ±
#[allow(dead_code)]
struct StorageLayout {
    /// slot0 storage slot indeksi
    slot0_index: RevmU256,
    /// liquidity storage slot indeksi
    liquidity_index: RevmU256,
    /// slot0'da unlocked flag'inin bit pozisyonu (None = ayrÄ± slot'ta)
        unlocked_bit_in_slot0: Option<u32>,
    /// PCS V3 gibi unlocked ayrÄ± slot'taysa: (slot_index, bit_pozisyonu)
    unlocked_separate_slot: Option<(RevmU256, u32)>,
}

impl StorageLayout {
    /// DEX tÃ¼rÃ¼ne gÃ¶re doÄŸru storage layout'u dÃ¶ndÃ¼r
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
                unlocked_bit_in_slot0: None, // slot0'a SIÄMAZ (272 bit > 256)
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

    /// Bu layout'a gÃ¶re slot0 ve iliÅŸkili storage slot'larÄ±nÄ± DB'ye yaz
    fn inject_slot0(&self, db: &mut InMemoryDB, addr: RevmAddress, sqrt_price_x96: U256, tick: i32, dex: DexType) {
        let slot0_value = pack_slot0(sqrt_price_x96, tick, dex);
        let _ = db.insert_account_storage(addr, self.slot0_index, slot0_value);

        // PCS V3 gibi unlocked ayrÄ± slot'taysa onu da yaz
        if let Some((slot_idx, _bit)) = &self.unlocked_separate_slot {
            let _ = db.insert_account_storage(addr, *slot_idx, pack_pcs_v3_slot1_unlocked());
        }
    }

    /// Bu layout'a gÃ¶re liquidity storage slot'unu DB'ye yaz
    fn inject_liquidity(&self, db: &mut InMemoryDB, addr: RevmAddress, liquidity: u128) {
        let _ = db.insert_account_storage(addr, self.liquidity_index, RevmU256::from(liquidity));
    }
}

/// Uniswap V3 / PancakeSwap V3 / Aerodrome slot0 storage paketleme.
///
/// v17.0 KRÄ°TÄ°K DÃœZELTME: DEX'e Ã¶zel storage layout farklarÄ±.
///
/// UniV3 slot0 (7 alan, uint8 feeProtocol â€” 248 bit, TEK slot):
///   [bits 0..159]   sqrtPriceX96 (uint160)
///   [bits 160..183] tick (int24, two's complement)
///   [bits 184..199] observationIndex (uint16) â€” 0
///   [bits 200..215] observationCardinality (uint16) â€” 0
///   [bits 216..231] observationCardinalityNext (uint16) â€” 0
///   [bits 232..239] feeProtocol (uint8) â€” 0
///   [bits 240..247] unlocked (bool) â€” TRUE
///
/// PancakeSwap V3 slot0 (7 alan, uint32 feeProtocol â€” 272 bit > 256, Ä°KÄ° slot!):
///   Storage Slot N:   sqrtPriceX96 + tick + observation alanlarÄ± (232 bit)
///   Storage Slot N+1: feeProtocol (uint32, bit 0-31) + unlocked (bool, bit 32)
///   Ã–NCEKÄ° BUG: unlocked slot N'de bit 240'a yazÄ±lÄ±yordu â†’ Pool Locked revert!
///
/// Aerodrome CLPool slot0 (6 alan, feeProtocol YOK â€” 240 bit, TEK slot):
///   [bits 0..159]   sqrtPriceX96 (uint160)
///   [bits 160..183] tick (int24)
///   [bits 184..231] observation alanlarÄ± (48 bit)
///   [bits 232..239] unlocked (bool) â€” TRUE
fn pack_slot0(sqrt_price_x96: U256, tick: i32, dex: DexType) -> RevmU256 {
    let mut packed = U256::ZERO;

    // sqrtPriceX96 â€” lower 160 bits
    let mask_160 = (U256::from(1u64) << 160) - U256::from(1u64);
    packed = packed | (sqrt_price_x96 & mask_160);

    // tick â€” int24 at bit 160 (two's complement, masked to 24 bits)
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

    // unlocked = true (1) â€” position depends on DEX type
    // PCS V3: uint32 feeProtocol (32 bit) slot 0'a SIÄMAZ (232+32=264 > 256)
    //         feeProtocol + unlocked â†’ slot N+1'e taÅŸar
    //         Bu yÃ¼zden slot 0'a unlocked yazÄ±lMAZ
    match dex {
        DexType::PancakeSwapV3 => {
            // PCS V3: unlocked slot 0'da deÄŸil, slot N+1'de (bit 32)
            // Burada sadece sqrtPriceX96 + tick yazÄ±lÄ±r
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
/// PCS V3 Slot0 struct'Ä±nda uint32 feeProtocol 256 bit sÄ±nÄ±rÄ±nÄ± aÅŸtÄ±ÄŸÄ± iÃ§in
/// feeProtocol + unlocked ayrÄ± bir slot'a taÅŸar:
///   [bits 0..31]  feeProtocol (uint32) â€” 0 yazÄ±lÄ±r
///   [bits 32..39] unlocked (bool) â€” TRUE olmalÄ±
fn pack_pcs_v3_slot1_unlocked() -> RevmU256 {
    to_revm_u256(U256::from(1u64) << 32)
}

// â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
// SimÃ¼lasyon Motoru
// â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

/// SimÃ¼lasyon motoru â€” havuz durumlarÄ±nÄ± REVM veritabanÄ±na yÃ¼kler
///
/// v10.0 Singleton Mimarisi:
///   - base_db: Bot baÅŸlatÄ±ldÄ±ÄŸÄ±nda bir kez oluÅŸturulur (bytecode + hesaplar)
///   - Her blokta base_db klonlanÄ±r, sadece slot0/liquidity gÃ¼ncellenir
///   - Bytecode her dÃ¶ngÃ¼de yeniden yÃ¼klenmez â†’ ~2-3ms tasarruf
pub struct SimulationEngine {
    /// Havuz bytecode Ã¶nbellekleri (adres â†’ bytecode)
    bytecode_cache: Vec<(Address, Vec<u8>)>,
    /// v22.1: Arbitraj kontrat bytecode'u (zincirden alÄ±nmÄ±ÅŸ)
    /// build_db'de kontrat hesabÄ±na yÃ¼klenir â€” simÃ¼lasyon gerÃ§ekÃ§i olur
    contract_bytecode: Option<Vec<u8>>,
    /// v22.1: Zincir ID'si (config'den alÄ±nÄ±r, hardcoded deÄŸil)
    chain_id: u64,
    /// v10.0: KalÄ±cÄ± temel veritabanÄ± (bytecode + hesaplar yÃ¼klÃ¼)
    /// Her simulate() Ã§aÄŸrÄ±sÄ±nda klonlanÄ±r, sadece slot'lar gÃ¼ncellenir
    base_db: Option<InMemoryDB>,
    /// base_db'deki caller ve contract adresleri
    base_caller: Option<Address>,
    base_contract: Option<Address>,
}

impl SimulationEngine {
    /// Yeni SimulationEngine oluÅŸtur
    pub fn new() -> Self {
        Self {
            bytecode_cache: Vec::new(),
            contract_bytecode: None,
            chain_id: 8453, // VarsayÄ±lan: Base
            base_db: None,
            base_caller: None,
            base_contract: None,
        }
    }

    /// v22.1: Zincir ID'sini ayarla (config'den)
    pub fn set_chain_id(&mut self, chain_id: u64) {
        self.chain_id = chain_id;
    }

    /// v22.1: Kontrat bytecode'unu ayarla (zincirden alÄ±nmÄ±ÅŸ)
    pub fn set_contract_bytecode(&mut self, bytecode: Vec<u8>) {
        self.contract_bytecode = Some(bytecode);
    }

    /// Havuz bytecode'larÄ±nÄ± Ã¶nbelleÄŸe al
    ///
    /// v25.0: Append-only mode â€” mevcut cache temizlenmez, yeni havuzlar eklenir.
    /// Hot-reload sÄ±rasÄ±nda sadece yeni havuzlar (slice) ile Ã§aÄŸrÄ±labilir;
    /// clear() eski havuzlarÄ±n bytecode'larÄ±nÄ± siliyordu.
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

    /// v10.0: Temel veritabanÄ±nÄ± bir kez oluÅŸtur (bytecode + hesaplar)
    ///
    /// Bot baÅŸlatÄ±ldÄ±ÄŸÄ±nda cache_bytecodes() sonrasÄ± Ã§aÄŸrÄ±lÄ±r.
    /// Bytecode ve hesap bilgileri kalÄ±cÄ± olarak base_db'ye yÃ¼klenir.
    /// Sonraki simulate() Ã§aÄŸrÄ±larÄ±nda bu klonlanÄ±r â€” bytecode yeniden
    /// yÃ¼klenmez, sadece slot0 ve liquidity gÃ¼ncellenir.
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

    /// v10.0: base_db'yi klonla ve sadece deÄŸiÅŸen slot'larÄ± gÃ¼ncelle
    ///
    /// Bytecode zaten base_db'de mevcut â€” yeniden yÃ¼klenmez.
    /// Sadece slot0 (sqrtPriceX96) ve slot4 (liquidity) gÃ¼ncellenir.
    /// Performans: ~0.05ms (eski: ~2-3ms)
    fn build_db_from_base(
        &self,
        pools: &[PoolConfig],
        states: &[SharedPoolState],
    ) -> InMemoryDB {
        let mut db = self.base_db.as_ref().unwrap().clone();

        // v20.0: StorageLayout ÅŸablonu ile DEX-baÄŸÄ±msÄ±z storage injection
        for (config, state_lock) in pools.iter().zip(states.iter()) {
            let state = state_lock.read();
            let addr = to_revm_addr(config.address);
            let layout = StorageLayout::for_dex(config.dex);

            layout.inject_slot0(&mut db, addr, state.sqrt_price_x96, state.tick, config.dex);
            layout.inject_liquidity(&mut db, addr, state.liquidity);
        }

        db
    }

    /// InMemoryDB oluÅŸtur ve havuz durumlarÄ±nÄ± doldur
    fn build_db(
        &self,
        pools: &[PoolConfig],
        states: &[SharedPoolState],
        caller: Address,
        contract: Address,
    ) -> InMemoryDB {
        let mut db = InMemoryDB::default();

        // â”€â”€ Havuz KontratlarÄ±nÄ± YÃ¼kle â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
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

            // v20.0: StorageLayout ÅŸablonu ile DEX-baÄŸÄ±msÄ±z storage injection
            let layout = StorageLayout::for_dex(config.dex);
            layout.inject_slot0(&mut db, addr, state.sqrt_price_x96, state.tick, config.dex);
            layout.inject_liquidity(&mut db, addr, state.liquidity);
        }

        // â”€â”€ Caller HesabÄ± (Test ETH Bakiyesi) â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
        db.insert_account_info(
            to_revm_addr(caller),
            AccountInfo::from_balance(RevmU256::from(100_000_000_000_000_000_000u128)), // 100 ETH
        );

        // â”€â”€ Kontrat HesabÄ± â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
        // v22.1: Kontrat bytecode'u varsa yÃ¼kle â€” simÃ¼lasyon gerÃ§ekÃ§i olur.
        // Bytecode yoksa boÅŸ hesap (sadece gas tahmini olarak kullanÄ±lÄ±r).
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

    /// Arbitraj iÅŸlemini REVM'de simÃ¼le et
    ///
    /// SimÃ¼lasyon adÄ±mlarÄ±:
    ///   1. InMemoryDB'yi gÃ¼ncel havuz verileriyle doldur
    ///   2. EVM ortamÄ±nÄ± yapÄ±landÄ±r (caller, hedef, calldata, gas)
    ///   3. Ä°ÅŸlemi yerel olarak Ã§alÄ±ÅŸtÄ±r
    ///   4. Sonucu analiz et (Success/Revert/Halt)
    ///
    /// # Notlar
    /// - DÄ±ÅŸ RPC Ã§aÄŸrÄ±sÄ± YAPILMAZ â€” tamamen yerel
    /// - Ä°lk block iÃ§in ~0.5ms, sonraki bloklar iÃ§in <0.1ms
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
        // 1. VeritabanÄ±nÄ± oluÅŸtur
        // v10.0: base_db varsa klonla+gÃ¼ncelle (hÄ±zlÄ±), yoksa sÄ±fÄ±rdan oluÅŸtur (fallback)
        let db = if self.base_db.is_some() {
            self.build_db_from_base(pools, states)
        } else {
            self.build_db(pools, states, caller, contract_address)
        };

        // 2. EVM'yi yapÄ±landÄ±r ve Ã§alÄ±ÅŸtÄ±r
        // v10.0: Timestamp ve base_fee artÄ±k zincir verisinden dinamik olarak gelir.
        //        Eski: SystemTime::now() â†’ yanlÄ±ÅŸ zaman damgasÄ±, base_fee yok
        //        Yeni: block_header.timestamp ve block_header.base_fee_per_gas
        let mut evm = Evm::builder()
            .with_db(db)
            .with_spec_id(SpecId::CANCUN)
            .modify_cfg_env(|cfg| {
                cfg.chain_id = self.chain_id; // v22.1: config'den, hardcoded deÄŸil
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
                tx.nonce = None; // Nonce kontrolÃ¼nÃ¼ atla
            })
            .build();

        // 3. Ä°ÅŸlemi Ã§alÄ±ÅŸtÄ±r
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
                    error: Some(format!("EVM hatasÄ±: {:?}", e)),
                }
            }
        }
    }

    /// Basit matematiksel doÄŸrulama simÃ¼lasyonu
    ///
    /// Tam REVM simÃ¼lasyonu yerine hÄ±zlÄ± bir kontrol yapar:
    ///   - Havuz verileri geÃ§erli mi?
    ///   - GerÃ§ek token kapasitesi (Î”x/Î”y) yeterli mi?
    ///   - Fiyat makul aralÄ±kta mÄ±?
    ///
    /// v20.0 KRÄ°TÄ°K DÃœZELTME: Likidite kontrolÃ¼ artÄ±k L parametresi ile
    /// doÄŸrudan karÅŸÄ±laÅŸtÄ±rma YAPMAZ. L, token miktarÄ± deÄŸil, fiyat eÄŸrisi
    /// oranÄ±dÄ±r. Bunun yerine, SqrtPriceMath formÃ¼lleri (Î”x, Î”y) ile
    /// havuzun mevcut fiyatÄ±ndan hedef fiyata kadar absorbe edebileceÄŸi
    /// gerÃ§ek WETH miktarÄ± hesaplanÄ±r.
    ///
    /// Bu fonksiyon REVM'in eksik state nedeniyle hatalÄ± sonuÃ§ vereceÄŸi
    /// durumlar iÃ§in fallback olarak kullanÄ±lÄ±r.
    pub fn validate_mathematical(
        &self,
        pools: &[PoolConfig],
        states: &[SharedPoolState],
        buy_pool_idx: usize,
        sell_pool_idx: usize,
        amount_weth: f64,
    ) -> SimulationResult {
        // Temel doÄŸrulamalar
        let buy_state = states[buy_pool_idx].read();
        let sell_state = states[sell_pool_idx].read();

        // 1. Havuzlar aktif mi?
        if !buy_state.is_active() || !sell_state.is_active() {
            return SimulationResult {
                success: false,
                gas_used: 0,
                error: Some("Havuz(lar) aktif deÄŸil".into()),
            };
        }

        // 2. Fiyatlar makul aralÄ±kta mÄ±? (Anormal fiyat â†’ Ã¶nce kontrol et)
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

        // 4. v20.0: GerÃ§ek token kapasitesi kontrolÃ¼ (V3 fiyat eÄŸrisi matematiÄŸi)
        //    ESKÄ° (HATALI): amount_weth * 1e18 * 10.0 vs liquidity_f64
        //      â†’ L bir token miktarÄ± DEÄÄ°LDÄ°R, bu karÅŸÄ±laÅŸtÄ±rma her zaman
        //        geÃ§erli iÅŸlemleri "Yetersiz Likidite" olarak reddediyordu.
        //    YENÄ°: hard_liquidity_cap_weth() ile mevcut sqrtPriceX96'dan
        //          itibaren V3 SqrtPriceMath formÃ¼lleri (Î”x = LÂ·Q96Â·(1/âˆšP_target - 1/âˆšP)
        //          veya Î”y = LÂ·(âˆšP_target - âˆšP)/Q96) kullanÄ±larak havuzun
        //          gerÃ§ek absorbe edebileceÄŸi WETH miktarÄ± hesaplanÄ±r.
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

        // TÃ¼m kontroller geÃ§ti
        // v20.0: Dinamik gas tahmini â€” swap adÄ±mÄ± sayÄ±sÄ±na gÃ¶re.
        // Tipik V3 single-pool swap: ~130K gas
        // Ã‡apraz swap (2 havuz): ~260K gas
        // Tick geÃ§iÅŸi baÅŸÄ±na ~+20K gas ek yÃ¼k
        // Kontrat overhead (flash loan + callback): ~50K gas
        // Toplam tahmini: 260K + 50K = ~310K (minimum taban)
        let estimated_gas: u64 = {
            let base_gas: u64 = 310_000; // 2-havuz Ã§apraz swap baz gas
            // TickBitmap varsa tahmini tick geÃ§iÅŸi ekle
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

// â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
// Calldata Payload MÃ¼hendisliÄŸi â€” 134 Byte Kompakt Kodlama (v9.0 Kontrat)
// â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
//
// Kontrat v9.0 ile uyumlu 134-byte calldata formatÄ±:
//
//   Offset  Boy   Alan
//   â”€â”€â”€â”€â”€â”€  â”€â”€â”€â”€  â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
//   0x00    20B   Pool A adresi (UniV3 flash swap)
//   0x14    20B   Pool B adresi (Slipstream satÄ±ÅŸ)
//   0x28    20B   owedToken (flash loan'a geri Ã¶denen token adresi)
//   0x3C    20B   receivedToken (flash loan'dan alÄ±nan + slipstream'e Ã¶denen)
//   0x50    32B   Miktar (uint256, big-endian)
//   0x70     1B   UniV3 YÃ¶n (0=zeroForOne, 1=oneForZero)
//   0x71     1B   Slipstream YÃ¶n (0=zeroForOne, 1=oneForZero)
//   0x72    16B   minProfit (uint128, big-endian â€” sandviÃ§ korumasÄ±)
//   0x82     4B   deadlineBlock (uint32, big-endian â€” blok son kullanma)
//   â”€â”€â”€â”€â”€â”€  â”€â”€â”€â”€  â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
//   Toplam: 134B  (v8.0: 130B + 4B deadlineBlock)
//
// Gas tasarrufu: ABI'nin 260+ byte'Ä±na karÅŸÄ± ~%48 tasarruf
// GÃ¼venlik: minProfit + deadlineBlock ile MEV + stale TX korumasÄ±
// â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

/// 134-byte kompakt calldata kodla (kontrat v9.0 uyumlu)
///
/// # Parametreler
/// - `pool_a`: UniV3 havuzu (flash swap kaynaÄŸÄ±)
/// - `pool_b`: Slipstream havuzu (satÄ±ÅŸ hedefi)
/// - `owed_token`: Flash loan geri Ã¶demesi iÃ§in token adresi
/// - `received_token`: Flash loan'dan alÄ±nan token adresi
/// - `amount_in_wei`: Ä°ÅŸlem miktarÄ± (uint256, big-endian)
/// - `uni_direction`: UniV3 yÃ¶n (0=zeroForOne, 1=oneForZero)
/// - `aero_direction`: Slipstream yÃ¶n (0=zeroForOne, 1=oneForZero)
/// - `min_profit`: Minimum kÃ¢r eÅŸiÄŸi (uint128, wei cinsinden)
/// - `deadline_block`: Son geÃ§erli blok numarasÄ± (uint32)
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

    // [0x00..0x14] Pool A adresi (UniV3) â€” 20 byte
    calldata.extend_from_slice(pool_a.as_slice());

    // [0x14..0x28] Pool B adresi (Slipstream) â€” 20 byte
    calldata.extend_from_slice(pool_b.as_slice());

    // [0x28..0x3C] owedToken adresi â€” 20 byte
    calldata.extend_from_slice(owed_token.as_slice());

    // [0x3C..0x50] receivedToken adresi â€” 20 byte
    calldata.extend_from_slice(received_token.as_slice());

    // [0x50..0x70] Miktar â€” uint256, 32 byte big-endian
    calldata.extend_from_slice(&amount_in_wei.to_be_bytes::<32>());

    // [0x70] UniV3 YÃ¶n â€” 1 byte
    calldata.push(uni_direction);

    // [0x71] Slipstream YÃ¶n â€” 1 byte
    calldata.push(aero_direction);

    // [0x72..0x82] minProfit â€” uint128, 16 byte big-endian
    calldata.extend_from_slice(&min_profit.to_be_bytes());

    // [0x82..0x86] deadlineBlock â€” uint32, 4 byte big-endian
    calldata.extend_from_slice(&deadline_block.to_be_bytes());

    debug_assert_eq!(calldata.len(), 134, "Kompakt calldata tam 134 byte olmalÄ±");
    calldata
}

/// Kompakt calldata'yÄ± Ã§Ã¶zÃ¼mle (test/debug iÃ§in)
///
/// 134 byte â†’ (pool_a, pool_b, owed_token, received_token, amount, uni_dir, aero_dir, min_profit, deadline_block)
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

/// Kompakt calldata'yÄ± hex string olarak formatla (log/debug)
pub fn format_compact_calldata_hex(calldata: &[u8]) -> String {
    format!("0x{}", hex::encode(calldata))
}

// â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
// Calldata Testleri (134-byte v9.0 formatÄ±)
// â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

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

        assert_eq!(calldata.len(), 134, "Kompakt calldata 134 byte olmalÄ±");
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

        // Ã‡Ã¶zÃ¼mle (round-trip)
        let (dec_a, dec_b, dec_owed, dec_recv, dec_amount, dec_uni, dec_aero, dec_profit, dec_deadline) =
            decode_compact_calldata(&calldata).expect("Decode baÅŸarÄ±sÄ±z");

        assert_eq!(dec_a, POOL_A, "Pool A adresi eÅŸleÅŸmeli");
        assert_eq!(dec_b, POOL_B, "Pool B adresi eÅŸleÅŸmeli");
        assert_eq!(dec_owed, USDC, "owedToken eÅŸleÅŸmeli");
        assert_eq!(dec_recv, WETH, "receivedToken eÅŸleÅŸmeli");
        assert_eq!(dec_amount, amount, "Miktar eÅŸleÅŸmeli");
        assert_eq!(dec_uni, 0x01, "UniV3 yÃ¶n eÅŸleÅŸmeli");
        assert_eq!(dec_aero, 0x00, "Slipstream yÃ¶n eÅŸleÅŸmeli");
        assert_eq!(dec_profit, min_profit, "minProfit eÅŸleÅŸmeli");
        assert_eq!(dec_deadline, deadline, "deadlineBlock eÅŸleÅŸmeli");
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

        // [0..20] = pool_a â†’ son byte 0x01
        assert_eq!(cd[19], 0x01, "Pool A son byte = 0x01");
        // [20..40] = pool_b â†’ son byte 0x02
        assert_eq!(cd[39], 0x02, "Pool B son byte = 0x02");
        // [40..60] = owedToken â†’ son byte 0x03
        assert_eq!(cd[59], 0x03, "owedToken son byte = 0x03");
        // [60..80] = receivedToken â†’ son byte 0x04
        assert_eq!(cd[79], 0x04, "receivedToken son byte = 0x04");
        // [80..112] = amount (32 byte big-endian, deÄŸer=1, son byte=0x01)
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
        // 133 byte (eksik) â€” decode None dÃ¶ndÃ¼rmeli
        let short = vec![0u8; 133];
        assert!(decode_compact_calldata(&short).is_none(), "133 byte reddedilmeli");

        // 135 byte (fazla) â€” decode None dÃ¶ndÃ¼rmeli
        let long = vec![0u8; 135];
        assert!(decode_compact_calldata(&long).is_none(), "135 byte reddedilmeli");

        // Eski 130 byte â€” reddedilmeli
        let old = vec![0u8; 130];
        assert!(decode_compact_calldata(&old).is_none(), "130 byte eski format reddedilmeli");

        // BoÅŸ veri
        let empty: Vec<u8> = vec![];
        assert!(decode_compact_calldata(&empty).is_none(), "BoÅŸ veri reddedilmeli");
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
        assert!(compact.len() < abi_size, "Kompakt format ABI'den kÃ¼Ã§Ã¼k olmalÄ±");

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

        // "0x" ile baÅŸlamalÄ±
        assert!(hex_str.starts_with("0x"), "Hex 0x ile baÅŸlamalÄ±");
        // 134 byte = 268 hex karakter + "0x" = 270 karakter
        assert_eq!(hex_str.len(), 270, "Hex string 270 karakter olmalÄ±");
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
            decode_compact_calldata(&calldata).expect("Decode baÅŸarÄ±sÄ±z");
        assert_eq!(decoded_profit, max_profit, "u128::MAX minProfit round-trip");
        assert_eq!(decoded_deadline, u32::MAX, "u32::MAX deadlineBlock round-trip");
    }

    #[test]
    fn test_real_base_scenario() {
        // GerÃ§ek Base Network senaryosu:
        // UniV3 WETH/USDC 0.05% havuzundan flash swap ile USDC al
        // Slipstream'de USDC ile WETH geri al
        // owedToken = WETH (UniV3'e geri Ã¶de)
        // receivedToken = USDC (UniV3'den al, Slipstream'e ver)
        let amount = U256::from(1_000_000_000_000_000_000u128); // 1 WETH
        let min_profit = 500_000u128; // 0.5 USDC (6 decimal)
        let deadline = 20_000_000u32; // Blok ~20M

        let calldata = encode_compact_calldata(
            POOL_A, POOL_B,
            WETH,    // owedToken (WETH borÃ§lu)
            USDC,    // receivedToken (USDC alÄ±nan)
            amount,
            0x01,   // UniV3: oneForZero (WETH al = zeroForOne=false â†’ direction=1)
            0x00,   // Slipstream: zeroForOne (USDC sat â†’ WETH al)
            min_profit,
            deadline,
        );

        assert_eq!(calldata.len(), 134);

        // Round-trip doÄŸrula  
        let decoded = decode_compact_calldata(&calldata).expect("Decode baÅŸarÄ±sÄ±z");
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

// â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
// L2 Sequencer Reorg & Stale State Testleri
// â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
//
// Base, tekil sequencer kullanan bir L2'dir. Sequencer yoÄŸunluk anlarÄ±nda
// reorg yapabilir â€” iyimser (optimistic) state gÃ¼ncellemesi yapÄ±lmÄ±ÅŸsa
// bot "hayalet fÄ±rsat" Ã¼zerine iÅŸlem gÃ¶nderebilir. Bu test modÃ¼lÃ¼,
// validate_mathematical fonksiyonunun bayat (stale) veya tutarsÄ±z state'leri
// doÄŸru ÅŸekilde reddettiÄŸini kanÄ±tlar.
// â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

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
        // v20.0: GerÃ§ekÃ§i sqrtPriceX96 â€” WETH/USDC (18dec/6dec) formatÄ±nda
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
    /// Senaryo: Bot pending TX'den iyimser state gÃ¼ncellemesi yaptÄ±.
    /// Sequencer bu TX'i dÃ¼ÅŸÃ¼rdÃ¼ (dropped). State artÄ±k bayat.
    /// validate_mathematical() 5000ms staleness eÅŸiÄŸini aÅŸan veriyi reddetmeli.
    #[test]
    fn test_sequencer_reorg_handling() {
        let pools = make_pool_configs();
        let sim = SimulationEngine::new();

        // Havuz A: taze state (henÃ¼z gÃ¼ncel)
        let state_a = make_active_state(2500.0, 10_000_000_000_000_000_000, 100);

        // Havuz B: bayat state (pending TX dÃ¼ÅŸÃ¼rÃ¼ldÃ¼ â†’ state gÃ¼ncellenmedi)
        // Son gÃ¼ncelleme 6 saniye Ã¶nceydi â†’ staleness_ms() > 5000
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
            last_block: 98, // 2 blok geride â€” reorg sonrasÄ±
            last_update: Instant::now() - Duration::from_secs(6), // 6s bayat
            is_initialized: true,
            bytecode: None,
            tick_bitmap: None,
            live_fee_bps: None,
        }));

        let states: Vec<SharedPoolState> = vec![state_a, state_b];

        // SimÃ¼lasyon: bayat state'li havuz â†’ BAÅARISIZ olmalÄ±
        let result = sim.validate_mathematical(&pools, &states, 0, 1, 1.0);
        assert!(!result.success, "Bayat (stale) state ile simÃ¼lasyon reddedilmeli");
        assert!(
            result.error.as_deref().unwrap_or("").contains("Bayat"),
            "Hata mesajÄ± 'Bayat' iÃ§ermeli, aldÄ±ÄŸÄ±mÄ±z: {:?}",
            result.error
        );
    }

    /// "Hayalet fÄ±rsat" testleri: Sequencer reorg sonrasÄ± geÃ§ersiz fiyatlar.
    ///
    /// Senaryo: Pending TX'den alÄ±nan fiyat 2500$, ancak TX dÃ¼ÅŸÃ¼rÃ¼ldÃ¼ÄŸÃ¼nde
    /// gerÃ§ek fiyat 0$ (veri yok/sÄ±fÄ±rlandÄ±). validate_mathematical bunu
    /// "Havuz aktif deÄŸil" olarak reddetmeli.
    #[test]
    fn test_sequencer_reorg_phantom_opportunity() {
        let pools = make_pool_configs();
        let sim = SimulationEngine::new();

        // Ä°yimser state: pending TX'den alÄ±nan fiyat ($2500)
        let state_a = make_active_state(2500.0, 10_000_000_000_000_000_000, 100);

        // Reorg sonrasÄ± havuz B: fiyat sÄ±fÄ±rlandÄ± (dropped TX)
        let state_b = Arc::new(RwLock::new(PoolState {
            sqrt_price_x96: U256::ZERO,
            sqrt_price_f64: 0.0,
            tick: 0,
            liquidity: 0, // Likidite de sÄ±fÄ±r
            liquidity_f64: 0.0,
            eth_price_usd: 0.0,
            last_block: 100,
            last_update: Instant::now(),
            is_initialized: false, // Havuz baÅŸlatÄ±lmamÄ±ÅŸ gibi
            bytecode: None,
            tick_bitmap: None,
            live_fee_bps: None,
        }));

        let states: Vec<SharedPoolState> = vec![state_a, state_b];

        let result = sim.validate_mathematical(&pools, &states, 0, 1, 1.0);
        assert!(!result.success, "Hayalet fÄ±rsat (phantom opportunity) reddedilmeli");
        assert!(
            result.error.as_deref().unwrap_or("").contains("aktif deÄŸil"),
            "Hata mesajÄ± 'aktif deÄŸil' iÃ§ermeli: {:?}",
            result.error
        );
    }

    /// Ã‡ift bayat state testi: Her iki havuz da stale.
    ///
    /// Senaryo: Sequencer tam kesintide, hiÃ§bir gÃ¼ncelleme gelmiyor.
    /// TÃ¼m havuzlar 10+ saniye bayat â†’ simÃ¼lasyon kesinlikle reddedilmeli.
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
            "Hata 'Bayat' iÃ§ermeli: {:?}",
            result.error
        );
    }

    /// Taze state â†’ simÃ¼lasyon baÅŸarÄ±lÄ± olmalÄ± (pozitif kontrol).
    #[test]
    fn test_fresh_state_passes_validation() {
        let pools = make_pool_configs();
        let sim = SimulationEngine::new();

        let states: Vec<SharedPoolState> = vec![
            make_active_state(2500.0, 10_000_000_000_000_000_000, 100),
            make_active_state(2520.0, 10_000_000_000_000_000_000, 100),
        ];

        let result = sim.validate_mathematical(&pools, &states, 0, 1, 1.0);
        assert!(result.success, "Taze state ile simÃ¼lasyon baÅŸarÄ±lÄ± olmalÄ±");
        assert!(result.error.is_none(), "Hata mesajÄ± olmamalÄ±");
    }

    /// Anormal fiyat testi: Reorg sonrasÄ± havuz fiyatÄ± saÃ§ma deÄŸere ulaÅŸmÄ±ÅŸ.
    #[test]
    fn test_sequencer_reorg_abnormal_price() {
        let pools = make_pool_configs();
        let sim = SimulationEngine::new();

        // Normal havuz
        let state_a = make_active_state(2500.0, 10_000_000_000_000_000_000, 100);
        // Reorg sonrasÄ± absÃ¼rd fiyat â€” flash loan manipÃ¼lasyonu veya veri bozulmasÄ±
        let state_b = make_active_state(999_999.0, 10_000_000_000_000_000_000, 100);

        let states: Vec<SharedPoolState> = vec![state_a, state_b];

        let result = sim.validate_mathematical(&pools, &states, 0, 1, 1.0);
        // 999,999 < 100,000 sÄ±nÄ±rÄ± aÅŸÄ±lÄ±yor â†’ anormal fiyat reddedilmeli
        assert!(!result.success, "Anormal fiyat ($999,999) reddedilmeli");
        assert!(
            result.error.as_deref().unwrap_or("").contains("Anormal fiyat"),
            "Hata 'Anormal fiyat' iÃ§ermeli: {:?}",
            result.error
        );
    }
}
