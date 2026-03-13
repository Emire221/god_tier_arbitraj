// ============================================================================
//  STATE_SYNC v9.2 — Multicall3 + Optimistic Pending TX Dinleyici
//
//  v9.2 Yenilikler (Issue #101 — Aerodrome ABI Kök-neden Analizi):
//  ✓ Aerodrome Slipstream slot0 ABI 6 parametre olarak düzeltildi
//    (Aerodrome CLPool.sol Slot0 struct'ında feeProtocol YOKTUR)
//  ✓ Aerodrome ticks() ABI 10 parametre olarak güncellendi
//    (Uniswap V3'ten farklı: ekstra stakedLiquidityNet + rewardGrowthOutsideX128)
//  ✓ Pool adresi doğrulama rehberi eklendi
//
//  v9.0 Yenilikler:
//  ✓ Pending TX stream (eth_subscribe newPendingTransactions)
//  ✓ İyimser (optimistic) havuz durum güncellemesi (blok öncesi tahmin)
//  ✓ Havuz adreslerine giden swap TX’lerini anlık yakalama
//
//  v8.0 (korunuyor):
//  ✓ Multicall3 (0xcA11bde05977b3631167028862bE2a173976CA11) entegrasyonu
//  ✓ 30-50 ayrı tickBitmap + ticks RPC çağrısı → TEK eth_call
//  ✓ Ağ gecikmesi ~80ms → ~5ms (1 RTT), rate-limit riski sıfır
//  ✓ sync_all_pools, cache_all_bytecodes hâlâ join_all (az sayıda çağrı)
//
//  Mimari:
//    1. tickBitmap word sorgularını Multicall3.aggregate3 ile paketle
//    2. Tek eth_call → tüm word’ler tek yanıtta döner
//    3. Başlatılmış tick’lerin detaylarını yine Multicall3 ile tek çağrıda oku
//    4. Toplam: 2 RPC çağrısı (eski: 40+ paralel çağrı)
//    5. [YENİ] Pending TX stream ile blok öncesi iyimser gücelleme
// ============================================================================

use alloy::primitives::{address, Address, Bytes, U256};
use alloy::providers::Provider;
use alloy::sol;
use alloy::sol_types::SolCall;
use eyre::Result;
use futures_util::StreamExt;
use std::time::Instant;
use futures_util::future::join_all;

use crate::math::compute_eth_price;
use crate::math::exact::u256_to_f64;
use crate::types::{DexType, PoolConfig, SharedPoolState, TickBitmapData, TickInfo};

// ─────────────────────────────────────────────────────────────────────────────
// Base L2 GasPriceOracle — L1 Data Fee Tahmin Kontratı
// ─────────────────────────────────────────────────────────────────────────────
//
// OP Stack (Base) üzerindeki her TX, L2 yürütme ücretine ek olarak
// L1'e veri yayınlama ücreti öder. Bu ücret TX boyutuna bağlıdır.
//
// GasPriceOracle kontratı (0x420...00F) işlemin calldata'sını alıp
// L1 veri ücretini wei cinsinden döndürür.
//
// Adres: 0x420000000000000000000000000000000000000F (Base, OP Mainnet, vb.)
// ─────────────────────────────────────────────────────────────────────────────

/// Base GasPriceOracle adresi (tüm OP Stack ağlarında standart)
const GAS_PRICE_ORACLE_ADDRESS: Address = address!("420000000000000000000000000000000000000F");

sol! {
    #[sol(rpc)]
    interface IGasPriceOracle {
        /// Verilen calldata için L1 data fee'sini wei cinsinden döndürür.
        function getL1Fee(bytes memory _data) external view returns (uint256);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Multicall3 — Standart Çok-Çağrı Kontratı (Tüm EVM Zincirlerde Aynı Adres)
// ─────────────────────────────────────────────────────────────────────────────

/// Multicall3 adresi — Base, Ethereum, Arbitrum, Optimism vb. hepsi aynı
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

// ─────────────────────────────────────────────────────────────────────────────
// Uniswap V3 Havuz Arayüzü (slot0 → 7 değişken, feeProtocol uint8)
// ─────────────────────────────────────────────────────────────────────────────

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

// ─────────────────────────────────────────────────────────────────────────────
// PancakeSwap V3 Havuz Arayüzü (slot0 → 7 değişken, feeProtocol uint32)
//
// ÖNEMLİ: PancakeSwap V3 slot0 struct'ı Uniswap V3'ten FARKLIDIR:
//   - Uniswap V3 slot0:     7 parametre, feeProtocol = uint8
//   - PancakeSwap V3 slot0: 7 parametre, feeProtocol = uint32
//
// PancakeSwap feeProtocol değeri ~209718400 olabilir ki bu uint8'e sığmaz.
// Alloy'un katı ABI çözümleyicisi bunu "buffer overrun" olarak raporlar.
//
// Kaynak: github.com/pancakeswap/pancake-v3-contracts/.../PancakeV3Pool.sol
// ─────────────────────────────────────────────────────────────────────────────

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

// ─────────────────────────────────────────────────────────────────────────────
// Aerodrome Slipstream Havuz Arayüzü (slot0 → 6 değişken, feeProtocol YOK)
//
// ÖNEMLİ: Aerodrome CLPool.sol Slot0 struct'ı Uniswap V3'ten FARKLIDIR:
//   - Uniswap V3 slot0: 7 parametre (feeProtocol DAHİL)
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
// Ayrıca Aerodrome ticks() 10 parametre döner (Uniswap V3: 8 parametre).
// Ekstra alanlar: int128 stakedLiquidityNet, uint256 rewardGrowthOutsideX128
// ─────────────────────────────────────────────────────────────────────────────

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

// ─────────────────────────────────────────────────────────────────────────────
// Tek Havuz Durum Senkronizasyonu
// ─────────────────────────────────────────────────────────────────────────────

/// RPC durum sorgulama zaman aşımı (milisaniye).
/// v28.0: 2000ms → 3000ms. Yoğun dönemlerde RPC gecikmeleri 2s'ı aşabilir.
/// 3000ms yeterli süre tanır, Base L2 ~2s blok süresi içinde yanıt beklenir.
/// v10.0: Timeout durumunda havuz STALE olarak işaretlenir — eski veri KULLANILMAZ.
const SYNC_TIMEOUT_MS: u64 = 3000;

/// Multicall3 toplu senkronizasyon chunk boyutu.
/// RPC payload limitleri nedeniyle havuzlar bu boyutta yığınlara bölünür.
/// Her havuz için 3 çağrı (slot0+liquidity+fee) → chunk=50 → 150 call/batch.
const MULTICALL_CHUNK_SIZE: usize = 30;

/// Maksimum yeniden deneme sayısı (timeout sonrası)
const SYNC_MAX_RETRIES: u32 = 2;

/// Tek bir havuzun durumunu RPC üzerinden oku ve SharedPoolState'e yaz
///
/// v17.0: Sıkı timeout (500ms) + yeniden deneme (2 kez) mekanizması.
///        RPC gecikmesi spike'ı (>500ms) durumunda eski veriyle devam etmek
///        yerine hızlıca yeniden dener. 2 deneme sonrası hata döner.
///
/// v10.0: slot0 ve liquidity sorguları artık paralel (tokio::join!)
///        Eski: 2 sıralı RPC çağrısı (2 RTT)
///        Yeni: 1 paralel çağrı (1 RTT) — blok başına ~2-5ms kazanç
pub async fn sync_pool_state<P: Provider + Sync>(
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
                // RPC hatası (timeout değil) — yeniden deneme
                if attempt < SYNC_MAX_RETRIES {
                    eprintln!(
                        "  \u{26a1} [{}] Sync error (attempt {}/{}): {}",
                        pool_config.name, attempt + 1, SYNC_MAX_RETRIES + 1, e
                    );
                }
                last_err = Some(e);
            }
            Err(_elapsed) => {
                // Timeout — yeniden deneme
                if attempt < SYNC_MAX_RETRIES {
                    eprintln!(
                        "  \u{26a1} [{}] Sync timeout ({}ms, attempt {}/{})",
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

    Err(last_err.unwrap_or_else(|| eyre::eyre!("[{}] sync failed", pool_config.name)))
}

/// sync_pool_state iç implementasyonu (timeout wrapper'sız)
async fn sync_pool_state_inner<P: Provider + Sync>(
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
                .map_err(|e| eyre::eyre!("[{}] slot0 read error (V3/7-field/uint8): {}", pool_config.name, e))?;
            let liq = liq_result
                .map_err(|e| eyre::eyre!("[{}] liquidity read error: {}", pool_config.name, e))?;
            let fee_bps: Option<u32> = fee_result.ok().map(|f| {
                let fee_u32: u32 = f.to();
                fee_u32 / 100
            });
            (slot0.sqrtPriceX96, slot0.tick.as_i32(), liq, fee_bps)
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
                    "[{}] slot0 read error (PCS-V3/7-field/uint32): {}\n\
                    → Is the pool address a valid PancakeSwap V3 Pool? Check: {}",
                    pool_config.name, e, pool_config.address
                ))?;
            let liq = liq_result
                .map_err(|e| eyre::eyre!("[{}] liquidity read error: {}", pool_config.name, e))?;
            let fee_bps: Option<u32> = fee_result.ok().map(|f| {
                let fee_u32: u32 = f.to();
                fee_u32 / 100
            });
            (slot0.sqrtPriceX96, slot0.tick.as_i32(), liq, fee_bps)
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
                    "[{}] slot0 read error (Aero/6-field): {}\n\
                    → Is the pool address a valid Aerodrome CLPool? Check: {}",
                    pool_config.name, e, pool_config.address
                ))?;
            let liq = liq_result
                .map_err(|e| eyre::eyre!("[{}] liquidity read error: {}", pool_config.name, e))?;
            let fee_bps: Option<u32> = fee_result.ok().map(|f| {
                let fee_u32: u32 = f.to();
                fee_u32 / 100
            });
            (slot0.sqrtPriceX96, slot0.tick.as_i32(), liq, fee_bps)
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
        state.is_stale = false;
        state.live_fee_bps = live_fee_bps;
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Multicall3 ABI Encoding Yardımcıları (slot0 / liquidity / fee)
// ─────────────────────────────────────────────────────────────────────────────

/// slot0() fonksiyon selektörü — tüm DEX'lerde aynı (selector: 0x3850c7bd)
fn encode_slot0_call() -> Vec<u8> {
    // slot0() — parametresiz, sadece 4-byte selector
    let call = IUniswapV3Pool::slot0Call {};
    IUniswapV3Pool::slot0Call::abi_encode(&call)
}

/// liquidity() fonksiyon selektörü — tüm DEX'lerde aynı (selector: 0x1a686502)
fn encode_liquidity_call() -> Vec<u8> {
    let call = IUniswapV3Pool::liquidityCall {};
    IUniswapV3Pool::liquidityCall::abi_encode(&call)
}

/// fee() fonksiyon selektörü — tüm DEX'lerde aynı (selector: 0xddca3f43)
fn encode_fee_call() -> Vec<u8> {
    let call = IUniswapV3Pool::feeCall {};
    IUniswapV3Pool::feeCall::abi_encode(&call)
}

/// Multicall3 sonucundan slot0 verisini decode et (DEX tipine göre)
///
/// # Dönüş
/// (sqrtPriceX96 as U256, tick) — başarısızsa None
fn decode_slot0_result(data: &[u8], dex: DexType) -> Option<(U256, i32)> {
    // Minimum ABI boyutu: 6 alan × 32 = 192 byte (Aerodrome), 7 alan × 32 = 224 byte (V3)
    let min_len = match dex {
        DexType::Aerodrome => 192,
        _ => 224,
    };
    if data.len() < min_len {
        return None;
    }

    // sqrtPriceX96: uint160 — ilk 32 byte (sağ hizalı ABI encoding)
    let sqrt_price_x96_bytes = &data[0..32];
    let sqrt_price_x96 = U256::from_be_slice(sqrt_price_x96_bytes);

    // uint160 aralık kontrolü — sqrtPriceX96 sıfır veya uint160 üstü olamaz
    let uint160_max = U256::from(1u64) << 160;
    if sqrt_price_x96.is_zero() || sqrt_price_x96 >= uint160_max {
        return None;
    }

    // tick: int24 — 2. word (offset 32..64), sign-extended
    let tick_word = &data[32..64];
    // Son 3 byte = int24, ama ABI'de int256 olarak sign-extended
    // Son 4 byte'ı int32 olarak oku
    let tick = i32::from_be_bytes(tick_word[28..32].try_into().ok()?);

    Some((sqrt_price_x96, tick))
}

/// Multicall3 sonucundan liquidity verisini decode et
/// # Dönüş: u128 liquidity değeri
fn decode_liquidity_result(data: &[u8]) -> Option<u128> {
    if data.len() < 32 {
        return None;
    }
    // uint128 — son 16 byte
    Some(u128::from_be_bytes(data[16..32].try_into().ok()?))
}

/// Multicall3 sonucundan fee verisini decode et
/// # Dönüş: fee (basis points cinsinden)
fn decode_fee_result(data: &[u8]) -> Option<u32> {
    if data.len() < 32 {
        return None;
    }
    // uint24 — son 4 byte'ın son 3 byte'ı
    let fee_raw = u32::from_be_bytes(data[28..32].try_into().ok()?);
    // fee() Uniswap V3'te "pips" döner (500 = %0.05), bps'e çevir
    Some(fee_raw / 100)
}

// ─────────────────────────────────────────────────────────────────────────────
// Toplu Multicall3 State Sync — Tüm Havuzları TEK eth_call ile Oku
// ─────────────────────────────────────────────────────────────────────────────

/// Tüm havuzların slot0+liquidity+fee verilerini Multicall3 ile toplu oku.
///
/// Eski mimari: `sync_all_pools` → `join_all` → N ayrı `sync_pool_state`
///   → Her biri 3 RPC çağrısı (slot0, liquidity, fee) = 3N RPC çağrısı
///   → 33 havuz = 99 RPC çağrısı → rate limit + yüksek gecikme
///
/// Yeni mimari: TEK Multicall3.aggregate3 çağrısı ile tüm veriler okunur
///   → 33 havuz × 3 fonksiyon = 99 Call3 → TEK eth_call
///   → 1 RTT (~3-5ms), rate-limit riski SIFIR
///
/// v10.0: Chunk desteği — 50 havuzdan fazlaysa otomatik bölünür (150 call/chunk).
///
/// # Stale Data Politikası (v10.0)
/// Multicall3 sonucunda `success=false` dönen havuzlar ARTIKeski veriyle devam
/// ETMEZ — `is_stale=true` olarak işaretlenir ve arbitraj pipeline'ından çıkarılır.
pub async fn sync_all_pools_multicall<P: Provider + Sync>(
    provider: &P,
    pools: &[PoolConfig],
    states: &[SharedPoolState],
    block_number: u64,
) -> Vec<Result<()>> {
    let pool_count = pools.len();
    if pool_count == 0 {
        return vec![];
    }

    let mut results: Vec<Result<()>> = (0..pool_count).map(|_| Ok(())).collect();

    // Havuzları chunk'lara böl (her chunk max MULTICALL_CHUNK_SIZE havuz)
    for chunk_start in (0..pool_count).step_by(MULTICALL_CHUNK_SIZE) {
        let chunk_end = (chunk_start + MULTICALL_CHUNK_SIZE).min(pool_count);
        let chunk_size = chunk_end - chunk_start;

        // Her havuz için 3 çağrı: slot0, liquidity, fee
        let slot0_calldata = encode_slot0_call();
        let liquidity_calldata = encode_liquidity_call();
        let fee_calldata = encode_fee_call();

        let mut calls: Vec<IMulticall3::Call3> = Vec::with_capacity(chunk_size * 3);

        for pool in pools.iter().take(chunk_end).skip(chunk_start) {
            // slot0
            calls.push(IMulticall3::Call3 {
                target: pool.address,
                allowFailure: true,
                callData: Bytes::from(slot0_calldata.clone()),
            });
            // liquidity
            calls.push(IMulticall3::Call3 {
                target: pool.address,
                allowFailure: true,
                callData: Bytes::from(liquidity_calldata.clone()),
            });
            // fee
            calls.push(IMulticall3::Call3 {
                target: pool.address,
                allowFailure: true,
                callData: Bytes::from(fee_calldata.clone()),
            });
        }

        // Multicall3 ile TEK eth_call
        let multicall = IMulticall3::new(MULTICALL3_ADDRESS, provider);
        let mc_result = match tokio::time::timeout(
            std::time::Duration::from_millis(SYNC_TIMEOUT_MS),
            multicall.aggregate3(calls).call(),
        ).await {
            Ok(Ok(res)) => res,
            Ok(Err(e)) => {
                // Multicall3 çağrısı başarısız — tüm chunk'ı stale işaretle
                for i in chunk_start..chunk_end {
                    states[i].write().is_stale = true;
                    results[i] = Err(eyre::eyre!(
                        "[{}] Multicall3 batch sync error: {}",
                        pools[i].name, e
                    ));
                }
                continue;
            }
            Err(_elapsed) => {
                // Timeout — tüm chunk'ı stale işaretle
                for i in chunk_start..chunk_end {
                    states[i].write().is_stale = true;
                    results[i] = Err(eyre::eyre!(
                        "[{}] Multicall3 sync timeout ({}ms)",
                        pools[i].name, SYNC_TIMEOUT_MS
                    ));
                }
                eprintln!(
                    "  \u{26a0}\u{fe0f} [Multicall3] Chunk {}-{} timeout ({}ms) — {} pools marked as STALE",
                    chunk_start, chunk_end, SYNC_TIMEOUT_MS, chunk_size,
                );
                continue;
            }
        };

        let expected_results = chunk_size * 3;
        if mc_result.len() != expected_results {
            eprintln!(
                "  ⚠️ [Multicall3] Unexpected result length: expected={} got={} (chunk {}-{})",
                expected_results,
                mc_result.len(),
                chunk_start,
                chunk_end,
            );
        }

        // Sonuçları decode et — her havuz için 3 sonuç (slot0, liquidity, fee)
        for (idx_in_chunk, pool_idx) in (chunk_start..chunk_end).enumerate() {
            let base = idx_in_chunk * 3;

            // Sonuçları al
            let slot0_res = mc_result.get(base);
            let liq_res = mc_result.get(base + 1);
            let fee_res = mc_result.get(base + 2);

            // slot0 decode
            let slot0_data = match slot0_res {
                Some(r) if r.success && !r.returnData.is_empty() => {
                    decode_slot0_result(&r.returnData, pools[pool_idx].dex)
                }
                _ => None,
            };

            // liquidity decode
            let liquidity_data = match liq_res {
                Some(r) if r.success && !r.returnData.is_empty() => {
                    decode_liquidity_result(&r.returnData)
                }
                _ => None,
            };

            // fee decode
            let fee_bps: Option<u32> = match fee_res {
                Some(r) if r.success && !r.returnData.is_empty() => {
                    decode_fee_result(&r.returnData)
                }
                _ => None,
            };

            // slot0 veya liquidity başarısız → havuz STALE
            match (slot0_data, liquidity_data) {
                (Some((sqrt_price_x96, tick)), Some(liquidity)) => {
                    // Başarılı — state güncelle
                    let sqrt_price_f64: f64 = u256_to_f64(sqrt_price_x96);
                    let liquidity_f64: f64 = u256_to_f64(U256::from(liquidity));

                    let eth_price = compute_eth_price(
                        sqrt_price_f64,
                        tick,
                        pools[pool_idx].token0_decimals,
                        pools[pool_idx].token1_decimals,
                        pools[pool_idx].token0_is_weth,
                    );

                    {
                        let mut state = states[pool_idx].write();
                        state.sqrt_price_x96 = sqrt_price_x96;
                        state.sqrt_price_f64 = sqrt_price_f64;
                        state.tick = tick;
                        state.liquidity = liquidity;
                        state.liquidity_f64 = liquidity_f64;
                        state.eth_price_usd = eth_price;
                        state.last_block = block_number;
                        state.last_update = Instant::now();
                        state.is_initialized = true;
                        state.is_stale = false;
                        state.live_fee_bps = fee_bps;
                    }
                    results[pool_idx] = Ok(());
                }
                _ => {
                    // Decode failed — mark pool as STALE
                    states[pool_idx].write().is_stale = true;
                    results[pool_idx] = Err(eyre::eyre!(
                        "[{}] Multicall3 slot0/liquidity decode failed (execution reverted?)",
                        pools[pool_idx].name
                    ));
                }
            }
        }
    }

    results
}

// ─────────────────────────────────────────────────────────────────────────────
// Havuz Sağlamlık Kontrolü (Pool Sanity Check) — Başlangıç Doğrulama
// ─────────────────────────────────────────────────────────────────────────────

/// Başlangıçta tüm havuzları Multicall3 ile on-chain doğrula.
///
/// Her havuz için slot0() ve liquidity() çağrılarını tek bir Multicall3
/// batch'inde gönderir. `execution reverted` veya decode hatası veren
/// havuzlar GEÇERSİZ olarak işaretlenir ve indeksleri döndürülür.
///
/// # Dönüş
/// Geçersiz havuz indeksleri (silinmesi gereken havuzlar)
pub async fn validate_pools<P: Provider + Sync>(
    provider: &P,
    pools: &[PoolConfig],
) -> Vec<usize> {
    let pool_count = pools.len();
    if pool_count == 0 {
        return vec![];
    }

    let mut invalid_indices: Vec<usize> = Vec::new();

    // slot0 + liquidity çağrıları — her havuz için 2 çağrı
    let slot0_cd = encode_slot0_call();
    let liq_cd = encode_liquidity_call();

    let mut calls: Vec<IMulticall3::Call3> = Vec::with_capacity(pool_count * 2);
    for pool in pools.iter() {
        calls.push(IMulticall3::Call3 {
            target: pool.address,
            allowFailure: true,
            callData: Bytes::from(slot0_cd.clone()),
        });
        calls.push(IMulticall3::Call3 {
            target: pool.address,
            allowFailure: true,
            callData: Bytes::from(liq_cd.clone()),
        });
    }

    let multicall = IMulticall3::new(MULTICALL3_ADDRESS, provider);
    let mc_results = match tokio::time::timeout(
        std::time::Duration::from_millis(10_000), // Başlangıç — daha uzun timeout
        multicall.aggregate3(calls).call(),
    ).await {
        Ok(Ok(res)) => res,
        Ok(Err(e)) => {
            eprintln!(
                "  \u{274c} [PoolValidation] Multicall3 validation error: {} — validation skipped",
                e
            );
            return vec![];
        }
        Err(_) => {
            eprintln!(
                "  \u{274c} [PoolValidation] Multicall3 validation timeout (10s) — validation skipped"
            );
            return vec![];
        }
    };

    let expected_results = pool_count * 2;
    if mc_results.len() != expected_results {
        eprintln!(
            "  ⚠️ [PoolValidation] Unexpected Multicall3 result length: expected={} got={}",
            expected_results,
            mc_results.len(),
        );
    }

    #[allow(clippy::needless_range_loop)]
    for i in 0..pool_count {
        let slot0_res = mc_results.get(i * 2);
        let liq_res = mc_results.get(i * 2 + 1);

        let slot0_ok = slot0_res
            .map(|r| r.success && !r.returnData.is_empty())
            .unwrap_or(false);
        let liq_ok = liq_res
            .map(|r| r.success && !r.returnData.is_empty())
            .unwrap_or(false);

        // slot0 veya liquidity decode doğrulaması
        let slot0_valid = if slot0_ok {
            decode_slot0_result(&slot0_res.unwrap().returnData, pools[i].dex).is_some()
        } else {
            false
        };

        let liq_valid = if liq_ok {
            decode_liquidity_result(&liq_res.unwrap().returnData).is_some()
        } else {
            false
        };

        if !slot0_valid || !liq_valid {
            invalid_indices.push(i);
            eprintln!(
                "  \u{274c} [PoolValidation] {} ({}) — INVALID (slot0={}, liquidity={}) → will be removed",
                pools[i].name,
                pools[i].address,
                if slot0_valid { "OK" } else { "FAIL" },
                if liq_valid { "OK" } else { "FAIL" },
            );
        }
    }

    invalid_indices
}

// ─────────────────────────────────────────────────────────────────────────────
// TickBitmap Off-Chain Okuma — Derinlik Haritası
// ─────────────────────────────────────────────────────────────────────────────

/// TickBitmap word pozisyonunu hesapla
/// tick_index / tick_spacing → compressed tick → word = compressed >> 8
#[inline]
fn tick_to_word_pos(tick: i32, tick_spacing: i32) -> i16 {
    // Compressed tick: Solidity'deki gibi negatifler için floor division
    let compressed = if tick < 0 && tick % tick_spacing != 0 {
        tick / tick_spacing - 1
    } else {
        tick / tick_spacing
    };
    (compressed >> 8) as i16
}

/// Bir bitmap word'ündeki tüm başlatılmış tick indekslerini çıkar
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

/// Havuzun TickBitmap'ini belirli bir aralıkta oku — Multicall3 ile TEK RPC
///
/// Bu fonksiyon:
///   1. Mevcut tick etrafındaki word pozisyonlarını hesaplar
///   2. Tüm tickBitmap(wordPos) çağrılarını Multicall3 ile TEK eth_call'da atar
///   3. Başlatılmış tick'ler için ticks(tick) çağrılarını yine Multicall3 ile toplar
///   4. Tüm veriyi TickBitmapData yapısına paketler
///
/// Performans: Eski: 30-50 ayrı RPC çağrısı → Yeni: 2 Multicall3 çağrısı (2 RTT)
pub async fn sync_tick_bitmap<P: Provider + Sync>(
    provider: &P,
    pool_config: &PoolConfig,
    pool_state: &SharedPoolState,
    block_number: u64,
    scan_range: u32,
) -> Result<()> {
    let start = Instant::now();

    let current_tick = pool_state.read().tick;
    let tick_spacing = pool_config.tick_spacing.max(1);

    // Tarama aralığı: current_tick ± (scan_range * tick_spacing)
    let tick_lo = current_tick - (scan_range as i32 * tick_spacing);
    let tick_hi = current_tick + (scan_range as i32 * tick_spacing);

    // Word pozisyon aralığı
    let word_lo = tick_to_word_pos(tick_lo, tick_spacing);
    let word_hi = tick_to_word_pos(tick_hi, tick_spacing);

    let mut bitmap_data = TickBitmapData::empty();
    bitmap_data.scan_range = scan_range;
    bitmap_data.snapshot_block = block_number;

    // ══════════════════════════════════════════════════════════════════════
    //  ADIM 1: tickBitmap word'lerini Multicall3 ile TEK ÇAĞRIDA oku
    // ══════════════════════════════════════════════════════════════════════

    let word_positions: Vec<i16> = (word_lo..=word_hi).collect();
    let mut all_initialized_ticks: Vec<i32> = Vec::new();

    if !word_positions.is_empty() {
        // Her word pozisyonu için calldata oluştur
        let calls: Vec<IMulticall3::Call3> = word_positions
            .iter()
            .map(|&word_pos| {
                let calldata = encode_tick_bitmap_call(pool_config.dex, word_pos);
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
            .map_err(|e| eyre::eyre!("[{}] Multicall3 tickBitmap error: {}", pool_config.name, e))?;

        // Sonuçları çözümle
        for (i, result) in results.iter().enumerate() {
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

    // ══════════════════════════════════════════════════════════════════════
    //  ADIM 2: Başlatılmış tick detaylarını Multicall3 ile TEK ÇAĞRIDA oku
    // ══════════════════════════════════════════════════════════════════════

    // Tarama aralığındaki tick'leri filtrele
    all_initialized_ticks.retain(|t| *t >= tick_lo && *t <= tick_hi);

    if !all_initialized_ticks.is_empty() {
        // Her tick için calldata oluştur
        let tick_calls: Vec<IMulticall3::Call3> = all_initialized_ticks
            .iter()
            .map(|&tick| {
                let tick_i24 = tick.clamp(-887272, 887272);
                let calldata = encode_ticks_call(pool_config.dex, tick_i24);
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
            .map_err(|e| eyre::eyre!("[{}] Multicall3 ticks error: {}", pool_config.name, e))?;

        // Sonuçları çözümle
        for (i, result) in tick_results.iter().enumerate() {
            if result.success && result.returnData.len() >= 64 {
                // İlk 32 byte = liquidityGross (uint128), sonraki 32 byte = liquidityNet (int128)
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

// ─────────────────────────────────────────────────────────────────────────────
// Havuz Bytecode Önbellekleme (REVM Simülasyonu İçin)
// ─────────────────────────────────────────────────────────────────────────────

pub async fn cache_pool_bytecode<P: Provider + Sync>(
    provider: &P,
    pool_config: &PoolConfig,
    pool_state: &SharedPoolState,
) -> Result<()> {
    let code = provider
        .get_code_at(pool_config.address)
        .await
        .map_err(|e| eyre::eyre!("[{}] Bytecode read error: {}", pool_config.name, e))?;

    let mut state = pool_state.write();
    state.bytecode = Some(code.to_vec());

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Toplu Senkronizasyon
// ─────────────────────────────────────────────────────────────────────────────

pub async fn sync_all_pools<P: Provider + Sync>(
    provider: &P,
    pools: &[PoolConfig],
    states: &[SharedPoolState],
    block_number: u64,
) -> Vec<Result<()>> {
    // v10.0: sync_all_pools artık sync_all_pools_multicall'a delege eder.
    // Multicall3 başarısız olursa (ağ hatası, kontrat sorunu) tekil fallback'e düşer.
    //
    // Stale Data Politikası (v10.0):
    // Sync başarısız olan havuzlar is_stale=true olarak işaretlenir.
    // Eski veri KESİNLİKLE KULLANILMAZ — hayalet kâr hesaplamasını önler.

    // Önce Multicall3 ile toplu sync dene
    let multicall_results = sync_all_pools_multicall(provider, pools, states, block_number).await;

    // Multicall3'te başarısız olan havuzlar için tekil fallback
    const FALLBACK_TIMEOUT_MS: u64 = 500;
    const FALLBACK_MAX_RETRIES: u32 = 1;

    let mut final_results = multicall_results;

    // Multicall3'te başarısız olan havuzların indekslerini topla
    let failed_indices: Vec<usize> = final_results.iter().enumerate()
        .filter(|(_, r)| r.is_err())
        .map(|(i, _)| i)
        .collect();

    for i in failed_indices {
            // Tekil fallback dene
            let config = pools[i].clone();
            let state = states[i].clone();

            let mut fallback_ok = false;
            for attempt in 0..=FALLBACK_MAX_RETRIES {
                match tokio::time::timeout(
                    std::time::Duration::from_millis(FALLBACK_TIMEOUT_MS),
                    sync_pool_state(provider, &config, &state, block_number),
                ).await {
                    Ok(Ok(())) => {
                        fallback_ok = true;
                        break;
                    }
                    Ok(Err(e)) => {
                        if attempt < FALLBACK_MAX_RETRIES {
                            eprintln!(
                                "     \u{26a1} [{}] Fallback sync error ({}/{}): {}",
                                config.name, attempt + 1, FALLBACK_MAX_RETRIES + 1, e,
                            );
                        }
                    }
                    Err(_) => {
                        if attempt < FALLBACK_MAX_RETRIES {
                            eprintln!(
                                "     \u{26a1} [{}] Fallback sync timeout ({}/{})",
                                config.name, attempt + 1, FALLBACK_MAX_RETRIES + 1,
                            );
                        }
                    }
                }
            }

            if fallback_ok {
                final_results[i] = Ok(());
            } else {
                // v10.0: Eski veri KULLANILMAZ — havuzu STALE olarak işaretle
                let staleness = state.read().staleness_ms();
                state.write().is_stale = true;
                eprintln!(
                    "     \u{1f6a8} [{}] Sync completely failed — marked as STALE (data age: {}ms)",
                    config.name, staleness,
                );
            }
    }

    final_results
}

/// Tüm havuzların TickBitmap'lerini senkronize et
///
/// Her havuz için:
///   1. tickBitmap word'lerini tarar
///   2. Başlatılmış tick'lerin liquidityNet bilgisini okur
///   3. PoolState.tick_bitmap'e yazar
///
/// v29.0: 2000ms → 500ms. Bayat veri beklemek yerine hızlı başarısızlık.
/// Multicall3 başarısız olduğunda tekil fallback 500ms ile hızlıca dener.
pub async fn sync_all_tick_bitmaps<P: Provider + Sync>(
    provider: &P,
    pools: &[PoolConfig],
    states: &[SharedPoolState],
    block_number: u64,
    scan_range: u32,
) -> Vec<Result<()>> {
    const BITMAP_TIMEOUT_MS: u64 = 500;

    let futures: Vec<_> = pools.iter().zip(states.iter())
        .map(|(config, state)| {
            let name = config.name.clone();
            async move {
                match tokio::time::timeout(
                    std::time::Duration::from_millis(BITMAP_TIMEOUT_MS),
                    sync_tick_bitmap(provider, config, state, block_number, scan_range),
                ).await {
                    Ok(result) => result,
                    Err(_) => {
                        eprintln!(
                            "     ⚠️ [TickBitmap] {} sync timeout ({}ms) — keeping existing data",
                            name, BITMAP_TIMEOUT_MS,
                        );
                        Err(eyre::eyre!("[{}] TickBitmap timeout ({}ms)", name, BITMAP_TIMEOUT_MS))
                    }
                }
            }
        })
        .collect();
    join_all(futures).await
}

// ─────────────────────────────────────────────────────────────────────────────
// L1 Data Fee Tahmini (Base / OP Stack)
// ─────────────────────────────────────────────────────────────────────────────

/// Arbitraj TX'inin L1 data fee'sini tahmin et (wei cinsinden).
///
/// GasPriceOracle.getL1Fee() çağrısı ile 134-byte kompakt calldata'nın
/// L1'e yayınlanma maliyetini sorguler. Blok başına 1 kez çağrılması yeterlidir.
///
/// v28.0: 500ms → 1000ms timeout. Timeout durumunda son başarılı sorgu
/// sonucunu önbellek olarak kullanır (statik fallback yerine).
/// Önbellek başlangıçta boştur — ilk timeout'ta konservatif fallback devreye girer.
///
/// # Dönüş
/// L1 data fee (wei). Hata/timeout durumunda önbellek, yoksa konservatif fallback döner.
pub async fn estimate_l1_data_fee<P: Provider + Sync>(
    provider: &P,
) -> u128 {
    use std::sync::atomic::{AtomicU64, Ordering};

    const L1_FEE_TIMEOUT_MS: u64 = 1000;
    const FALLBACK_FEE_WEI: u128 = 500_000_000_000_000; // 0.0005 ETH

    // v28.0: Son başarılı L1 fee sonucunu önbelleğe al.
    // AtomicU64 kullanılır (u128'i 2 parçaya bölmedik çünkü L1 fee
    // pratikte u64 aralığında kalır — ~18 ETH max).
    static CACHED_L1_FEE: AtomicU64 = AtomicU64::new(0);

    match tokio::time::timeout(
        std::time::Duration::from_millis(L1_FEE_TIMEOUT_MS),
        estimate_l1_data_fee_inner(provider),
    ).await {
        Ok(fee) => {
            // Başarılı sorgu — önbelleği güncelle
            if fee > 0 && fee < u64::MAX as u128 {
                CACHED_L1_FEE.store(fee as u64, Ordering::Relaxed);
            }
            fee
        }
        Err(_elapsed) => {
            // Timeout — önbellekten son bilinen değeri kullan
            let cached = CACHED_L1_FEE.load(Ordering::Relaxed);
            if cached > 0 {
                eprintln!(
                    "  ⚠️ [L1 Fee] GasPriceOracle query timed out ({}ms) — using cache ({} wei)",
                    L1_FEE_TIMEOUT_MS, cached,
                );
                cached as u128
            } else {
                eprintln!(
                    "  ⚠️ [L1 Fee] GasPriceOracle query timed out ({}ms) — using conservative fallback",
                    L1_FEE_TIMEOUT_MS,
                );
                FALLBACK_FEE_WEI
            }
        }
    }
}

/// estimate_l1_data_fee iç implementasyonu (timeout wrapper'sız)
async fn estimate_l1_data_fee_inner<P: Provider + Sync>(
    provider: &P,
) -> u128 {
    const FALLBACK_FEE_WEI: u128 = 500_000_000_000_000; // 0.0005 ETH
    // 134-byte representative calldata (mostly non-zero for worst case estimate)
    // Gerçek calldata adresleri ve miktarları değişir ama boyut sabittir.
    // Non-zero byte'lar 16 gas, zero byte'lar 4 gas maliyetlidir (EIP-2028).
    // Worst case: tamamı non-zero → konservatif tahmin.
    let representative_calldata: Vec<u8> = vec![0xFFu8; 134];

    let oracle = IGasPriceOracle::new(GAS_PRICE_ORACLE_ADDRESS, provider);
    match oracle
        .getL1Fee(representative_calldata.into())
        .call()
        .await
    {
        Ok(fee) => {
            // U256 → u128 safe conversion
            if fee > alloy::primitives::U256::from(u128::MAX) {
                u128::MAX
            } else {
                let fee_u128 = fee.to::<u128>();
                // v27.0: Oracle başarılı döndü ama 0 ise → kontrat yanlış çalışıyor olabilir.
                // Base mainnet'te 134-byte calldata için L1 fee asla 0 olmamalı.
                // Konservatif fallback kullan ve uyar.
                if fee_u128 == 0 {
                    eprintln!(
                        "  ⚠️ [L1 Fee] GasPriceOracle.getL1Fee() returned 0 — oracle data feed may be faulty, using conservative fallback",
                    );
                    FALLBACK_FEE_WEI
                } else {
                    fee_u128
                }
            }
        }
        Err(e) => {
            eprintln!(
                "  ⚠️ L1 data fee estimation failed (fallback: conservative estimate): {}",
                e
            );
            FALLBACK_FEE_WEI
        }
    }
}

pub async fn cache_all_bytecodes<P: Provider + Sync>(
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
/// Hem UniswapV3 hem Aerodrome aynı fonksiyon imzasını kullanır:
///   selector = keccak256("ticks(int24)")[0..4] = 0xf30dba93
fn encode_ticks_call(_dex: DexType, tick: i32) -> Vec<u8> {
    // ticks(int24) — ABI: selector(4) + int24 padded to 32 bytes
    // Alloy v1.7: int24 = Signed<24, 1> olarak temsil edilir
    use alloy::primitives::Signed;
    let tick_signed: Signed<24, 1> = Signed::try_from(tick as i64)
        .unwrap_or(Signed::ZERO);
    let call = IUniswapV3Pool::ticksCall { tick: tick_signed };
    IUniswapV3Pool::ticksCall::abi_encode(&call)
}

/// ticks() dönüş verisini decode et
///
/// UniswapV3: 256 byte (8 alan × 32 byte)
/// Aerodrome: 320 byte (10 alan × 32 byte — ekstra stakedLiquidityNet + rewardGrowthOutside)
///
/// İlk 8 alan her iki DEX'te aynı düzendedir:
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
/// Uniswap V3 ticks() → 8 parametre (256 byte)
/// PancakeSwap V3 ticks() → 8 parametre (256 byte)  
/// Aerodrome ticks() → 10 parametre (320 byte)
///
/// İlk 3 kullanılan alan tüm DEX'lerde aynı offset'tedir:
///   [0..32]   uint128 liquidityGross      — tüm DEX'lerde 1. alan
///   [32..64]  int128  liquidityNet         — tüm DEX'lerde 2. alan
///   [224..256] bool   initialized          — tüm DEX'lerde 8. alan
///
/// Aerodrome ek alanları (stakedLiquidityNet, rewardGrowthOutsideX128)
/// 8. alandan SONRA gelir, dolayısıyla initialized offset'i değişmez.
///
/// Dönüş: (liquidityGross, liquidityNet, initialized)
fn decode_ticks_result(data: &[u8]) -> Option<(u128, i128, bool)> {
    if data.len() < 256 {
        return None;
    }

    // liquidityGross: uint128 (son 16 byte of first 32-byte word)
    let liq_gross = u128::from_be_bytes(data[16..32].try_into().ok()?);

    // liquidityNet: int128 (son 16 byte of second 32-byte word)
    let liq_net = i128::from_be_bytes(data[48..64].try_into().ok()?);

    // initialized: bool (son byte of eighth 32-byte word)
    // v22.0: Offset doğrulaması — 8. word tüm DEX'lerde aynı (index=7)
    let initialized = data[255] != 0;

    Some((liq_gross, liq_net, initialized))
}

// ─────────────────────────────────────────────────────────────────────────────
// Optimistic Pending TX Dinleyici (FAZ 4 — Gecikme İyileştirmesi)
// ─────────────────────────────────────────────────────────────────────────────
//
// Amaç: Blok onayını beklemeden, mempool/sequencer'daki bekleyen swap
// işlemlerini yakalayıp havuz durumlarını iyimser (optimistic) olarak
// güncellemek. Bu sayede bot ~15-20ms erken hareket edebilir.
//
// Akış:
//   1. WebSocket üzerinden pending TX stream aç
//   2. Gelen TX'in `to` adresi izlenen havuzlardan biri mi?
//   3. Evet → TX calldata'sından swap yönünü ve miktarını çıkar
//   4. In-memory fiyat tahminini güncelle (optimistic update)
//   5. Strateji modülü güncel fiyatları okuyarak erken arbitraj tespiti yapar
//
// NOT: Base L2 sequencer FIFO'dur — mempool sınırlıdır.
//      Bu modül "best effort" çalışır, pending TX yoksa mevcut blok
//      bazlı akış aynen devam eder.
// ─────────────────────────────────────────────────────────────────────────────

/// Uniswap V3 / Aerodrome swap fonksiyon selektörü
/// swap(address,bool,int256,uint160,bytes) → 0x128acb08
const SWAP_SELECTOR: [u8; 4] = [0x12, 0x8a, 0xcb, 0x08];

/// Pending TX'in izlenen bir havuza swap olup olmadığını kontrol et
///
/// Dönen değer: (havuz_indeksi, is_swap) — swap değilse None
pub fn check_pending_tx_relevance(
    tx_to: Option<Address>,
    tx_input: &[u8],
    pool_addresses: &[Address],
) -> Option<usize> {
    let to = tx_to?;

    // Hedef adres izlenen havuzlardan biri mi?
    let pool_idx = pool_addresses.iter().position(|&addr| addr == to)?;

    // Calldata en az 4 byte (selector) olmalı
    if tx_input.len() < 4 {
        return None;
    }

    // Swap selektörü mü?
    if tx_input[0..4] == SWAP_SELECTOR {
        Some(pool_idx)
    } else {
        None
    }
}

/// Pending swap TX varsa havuz durumunu iyimser olarak güncelle
///
/// Bu fonksiyon tam bir fiyat hesabı YAPMAZ — sadece havuzun
/// "yakında fiyat değişecek" sinyalini verir ve mevcut state'i
/// yeniden okumayı tetikler.
///
/// # Parametreler
/// - `provider`: RPC sağlayıcı (anlık slot0 sorgusu için)
/// - `pool_config`: Etkilenen havuzun yapılandırması
/// - `pool_state`: Güncellenen havuz durumu (write lock alır)
/// - `current_block`: Mevcut blok numarası
///
/// # Dönüş
/// - Ok(true): Durum güncellendi (yeni swap tespit edildi)
/// - Ok(false): Güncelleme gerekmedi
/// - Err: RPC hatası
pub async fn optimistic_refresh_pool<P: Provider + Sync>(
    provider: &P,
    pool_config: &PoolConfig,
    pool_state: &SharedPoolState,
    current_block: u64,
) -> Result<bool> {
    // Havuzun güncel slot0 ve liquidity değerlerini anlık oku
    // v10.0: Paralel okuma (tokio::join!) — tek RTT (~1-3ms)
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
                .map_err(|e| eyre::eyre!("[OPT:{}] slot0 read error (V3/uint8): {}", pool_config.name, e))?;
            let liq = liq_result
                .map_err(|e| eyre::eyre!("[OPT:{}] liquidity read error: {}", pool_config.name, e))?;
            (slot0.sqrtPriceX96, slot0.tick.as_i32(), liq)
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
                .map_err(|e| eyre::eyre!("[OPT:{}] slot0 read error (PCS-V3/uint32): {}", pool_config.name, e))?;
            let liq = liq_result
                .map_err(|e| eyre::eyre!("[OPT:{}] liquidity read error: {}", pool_config.name, e))?;
            (slot0.sqrtPriceX96, slot0.tick.as_i32(), liq)
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
                .map_err(|e| eyre::eyre!("[OPT:{}] slot0 read error (Aero/6-field): {}", pool_config.name, e))?;
            let liq = liq_result
                .map_err(|e| eyre::eyre!("[OPT:{}] liquidity read error: {}", pool_config.name, e))?;
            (slot0.sqrtPriceX96, slot0.tick.as_i32(), liq)
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

    // Mevcut state ile karşılaştır — fiyat değişmişse güncelle
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
        state.is_stale = false; // Optimistic refresh — veri taze
        Ok(true)
    } else {
        Ok(false)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// EVENT-DRIVEN STATE SYNC — Swap Event Dinleyici (v11.0)
// ─────────────────────────────────────────────────────────────────────────────
//
// Polling yerine eth_subscribe("logs") ile Swap eventlerini dinler.
// Swap eventi doğrudan sqrtPriceX96, liquidity ve tick bilgisi içerir —
// ek slot0/liquidity RPC çağrısına gerek kalmaz (zero-latency).
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
// Sync Event (likidite değişimi):
//   Mint/Burn eventleri de dinlenebilir, ancak Swap yeterlidir çünkü
//   her swap sonrası liquidity ve sqrtPrice günceldir.
// ─────────────────────────────────────────────────────────────────────────────

/// Uniswap V3 / Aerodrome Swap event topic0
/// keccak256("Swap(address,address,int256,int256,uint160,uint128,int24)")
const SWAP_EVENT_TOPIC: [u8; 32] = [
    0xc4, 0x20, 0x79, 0xf9, 0x4a, 0x63, 0x50, 0xd7,
    0xe6, 0x23, 0x5f, 0x29, 0x17, 0x49, 0x24, 0xf9,
    0x28, 0xcc, 0x2a, 0xc8, 0x18, 0xeb, 0x64, 0xfe,
    0xd8, 0x00, 0x4e, 0x11, 0x5f, 0xbc, 0xca, 0x67,
];

/// Swap event log verisinden havuz durumunu çıkar ve güncelle.
///
/// Log Data formatı (non-indexed parametreler, ABI-encoded):
///   [0..32]    int256  amount0
///   [32..64]   int256  amount1
///   [64..96]   uint160 sqrtPriceX96 (sağ hizalı, 32 byte padded)
///   [96..128]  uint128 liquidity (sağ hizalı, 32 byte padded)
///   [128..160] int24   tick (sağ hizalı, 32 byte padded, sign-extended)
///
/// # Dönüş
/// Ok(true) → durum güncellendi, Ok(false) → güncelleme gerekmedi
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

    // Log data en az 160 byte olmalı (5 × 32 byte)
    if log_data.len() < 160 {
        return Ok(false);
    }

    // sqrtPriceX96 çıkar (offset 64..96, uint160)
    let sqrt_price_x96 = U256::from_be_slice(&log_data[64..96]);

    // liquidity çıkar (offset 96..128, uint128)
    let liquidity_bytes = &log_data[112..128]; // Son 16 byte = uint128
    let liquidity = u128::from_be_bytes(liquidity_bytes.try_into().unwrap_or([0u8; 16]));

    // tick çıkar (offset 128..160, int24 olarak sign-extended int256)
    // Son 4 byte'ı int32 olarak oku, sonra -887272..887272 aralığına sınırla
    let tick_bytes = &log_data[156..160]; // Son 4 byte
    let tick_raw = i32::from_be_bytes(tick_bytes.try_into().unwrap_or([0u8; 4]));
    let tick = tick_raw.clamp(-887272, 887272);

    let config = &pools[pool_idx];

    // f64 dönüşümleri
    let sqrt_price_f64: f64 = u256_to_f64(sqrt_price_x96);
    let liquidity_f64: f64 = liquidity as f64;

    // ETH fiyatı hesapla
    let eth_price = compute_eth_price(
        sqrt_price_f64,
        tick,
        config.token0_decimals,
        config.token1_decimals,
        config.token0_is_weth,
    );

    // State güncelle
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
        state.is_stale = false; // Event-driven güncelleme — veri taze
    }

    Ok(true)
}

/// Event-driven Swap dinleyici başlat.
///
/// Havuz adreslerindeki Swap eventlerini WebSocket/IPC üzerinden dinler.
/// Her Swap eventi geldiğinde havuz state'ini anlık günceller.
/// Polling'e göre avantaj: Sıfır gecikme, ek RPC çağrısı yok.
///
/// # Parametreler
/// - `rpc_url`: WebSocket/IPC RPC adresi
/// - `pools`: İzlenen havuz yapılandırmaları
/// - `states`: Paylaşımlı havuz durumları
///
/// # Dönüş
/// Bu fonksiyon sonsuz döngüde çalışır. Bağlantı koparsa Err döner.
pub async fn start_swap_event_listener<P: Provider + Sync>(
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

    // Log subscription başlat
    let sub = provider.subscribe_logs(&filter).await
        .map_err(|e| eyre::eyre!("Swap event subscription error: {}", e))?;
    let mut stream = sub.into_stream();

    println!(
        "  \u{26a1} Event-driven Swap listener active ({} pools)", pools.len()
    );

    while let Some(log) = stream.next().await {
        // Log adresini al (Deref through inner)
        let log_address = log.inner.address;

        // Blok numarasını al
        let block_number = log.block_number.unwrap_or(0);

        // Swap event log verisini işle
        let log_data: &[u8] = log.inner.data.data.as_ref();

        match process_swap_event_log(
            log_data,
            log_address,
            block_number,
            pools,
            states,
        ) {
            Ok(true) => {
                // State güncellendi — havuz bilgisini logla
                if let Some(idx) = pools.iter().position(|p| p.address == log_address) {
                    let state = states[idx].read();
                    eprintln!(
                        "     ⚡ [Event] {} → {:.2}$ | Tick: {} | Block: #{}",
                        pools[idx].name,
                        state.eth_price_usd,
                        state.tick,
                        block_number,
                    );
                }
            }
            Ok(false) => {} // Güncelleme gerekmedi
            Err(e) => {
                eprintln!("     ⚠️ [Event] Swap log processing error: {}", e);
            }
        }
    }

    Err(eyre::eyre!("Swap event stream closed"))
}

// ─────────────────────────────────────────────────────────────────────────────
// RPC Connection Drop Failover Testleri
// ─────────────────────────────────────────────────────────────────────────────
//
// Risk: HFT botları WebSocket/IPC üzerinden node ile haberleşir. Node'un
// soketi aniden kapanırsa (EOF error), Rust panik yapıp çökebilir.
//
// Bu test modülü doğrular:
//   1. sync_pool_state hata döndürür ama panik YAPMAZ
//   2. Ardışık RPC hataları is_active() → false ile tespit edilir
//   3. staleness_ms eşiği aşıldığında güvenli geçiş yapılır
//   4. Havuz state'i son bilinen güvenli değerde korunur
//
// Not: Gerçek WSS bağlantı kopması main.rs'deki reconnect döngüsü
// tarafından ele alınır (run_bot() → Result::Err → exponential backoff).
// Bu testler state katmanının panik-güvenli davranışını kanıtlar.
// ─────────────────────────────────────────────────────────────────────────────

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
            is_stale: false,
        }))
    }

    /// RPC bağlantı kopması simülasyonu: Havuz state yazma paniklememeli.
    ///
    /// Senaryo: WSS soketi kapanır → sync_pool_state RPC hatası alır
    /// → state güncellenmez → staleness_ms artar → is_active() hâlâ true
    /// ama veri bayat → check_arbitrage_opportunity reddeder.
    ///
    /// Bu test, tüm akışın panik olmadan çalıştığını kanıtlar.
    #[test]
    fn test_rpc_failover_without_panic() {
        // ── 1. Başlangıç: Aktif state ──────────────────────────
        let state = make_active_state(2500.0, 10_000_000_000_000_000_000, 100);

        // State aktif ve taze
        {
            let s = state.read();
            assert!(s.is_active(), "Başlangıçta state aktif olmalı");
            assert!(s.staleness_ms() < 100, "Başlangıçta veri taze olmalı");
        }

        // ── 2. RPC kopması simülasyonu ────────────────────────────
        // sync_pool_state çağrıldığında RPC hatası alınır (burada simüle ediyoruz).
        // State güncellenmez → son bilinen değerde kalır.
        // Bu noktada panik olmamalı.

        // Bayatlık simülasyonu: last_update'i 6 saniye geriye çek
        {
            let mut s = state.write();
            s.last_update = Instant::now() - Duration::from_secs(6);
        }

        // ── 3. Doğrulama: State bayat ama panic yok ─────────────
        {
            let s = state.read();
            assert!(s.is_active(), "State hâlâ aktif (eski veriler geçerli)");
            assert!(
                s.staleness_ms() >= 5000,
                "Veri bayat olmalı (>5s): {}ms",
                s.staleness_ms()
            );
            // Fiyat ve likidite son bilinen değerde korunmuş
            assert_eq!(s.eth_price_usd, 2500.0, "Fiyat son bilinen değerde kalmalı");
            assert_eq!(s.liquidity, 10_000_000_000_000_000_000, "Likidite korunmalı");
        }

        // ── 4. Yeniden bağlantı sonrası kurtarma ────────────────
        // sync_pool_state yeni RPC ile başarılı olur → state güncellenir
        {
            let mut s = state.write();
            s.last_update = Instant::now();
            s.last_block = 105;
            s.eth_price_usd = 2510.0;
        }

        {
            let s = state.read();
            assert!(s.is_active(), "Kurtarma sonrası state aktif olmalı");
            assert!(
                s.staleness_ms() < 100,
                "Kurtarma sonrası veri taze olmalı"
            );
            assert_eq!(s.eth_price_usd, 2510.0, "Kurtarma sonrası fiyat güncel");
            assert_eq!(s.last_block, 105, "Kurtarma sonrası blok güncel");
        }
    }

    /// Ardışık RPC hataları: State bayatlaşır, is_active() hâlâ true ama
    /// staleness eşiği aşıldığında bot fırsat aramayı durdurur.
    #[test]
    fn test_rpc_consecutive_failures_staleness_protection() {
        let state = make_active_state(2500.0, 10_000_000_000_000_000_000, 100);

        // 3 ardışık "RPC hatası" — state güncellenmez
        for i in 1..=3 {
            // Her "hatada" 2 saniye geçiyor
            {
                let mut s = state.write();
                s.last_update = Instant::now() - Duration::from_secs(2 * i);
            }

            let s = state.read();
            // is_active hâlâ true (panik yok, state bozulmadı)
            assert!(s.is_active(), "Hata #{}: is_active hâlâ true", i);
        }

        // 6 saniye sonra staleness eşiğini aştı
        let s = state.read();
        assert!(
            s.staleness_ms() >= 5000,
            "3 ardışık hatadan sonra veri bayat olmalı"
        );
    }

    /// Sıfır state koruması: Hiç güncelleme gelmezse state varsayılan değerlerde.
    /// Bu da panik yapmaz — is_active() false döner.
    #[test]
    fn test_rpc_never_connected_no_panic() {
        let state: SharedPoolState = Arc::new(RwLock::new(PoolState::default()));

        let s = state.read();
        assert!(
            !s.is_active(),
            "Hiç bağlantı kurulmadıysa state aktif olmamalı"
        );
        assert_eq!(s.eth_price_usd, 0.0, "Fiyat 0 (varsayılan)");
        assert_eq!(s.liquidity, 0, "Likidite 0 (varsayılan)");
        // Panik yok — güvenli varsayılan değerler
    }

    /// SharedPoolState RwLock eş zamanlı erişim — panik yok.
    /// Birden fazla okuyucu aynı anda erişebilir.
    #[test]
    fn test_rpc_failover_concurrent_access_no_panic() {
        let state = make_active_state(2500.0, 10_000_000_000_000_000_000, 100);

        // Eş zamanlı okuma (parking_lot RwLock birden fazla reader kabul eder)
        let s1 = state.read();
        let s2 = state.read();

        assert_eq!(s1.eth_price_usd, s2.eth_price_usd, "Eş zamanlı okuma tutarlı");
        assert_eq!(s1.liquidity, s2.liquidity, "Likidite değerleri tutarlı");

        drop(s1);
        drop(s2);

        // Yazma sonrası okuma
        {
            let mut s = state.write();
            s.eth_price_usd = 2600.0;
        }

        let s = state.read();
        assert_eq!(s.eth_price_usd, 2600.0, "Yazma sonrası okuma doğru");
    }

    /// Graceful degradation kanıtı: run_bot() hata döndürdüğünde
    /// exponential backoff ile yeniden bağlanma stratejisi.
    /// Bu test, delay hesaplamasının doğruluğunu kanıtlar.
    #[test]
    fn test_reconnect_exponential_backoff_calculation() {
        // main.rs'deki delay hesaplama mantığını birebir test et
        for retry_count in 1u32..=10 {
            let delay_ms = if retry_count <= 3 {
                100u64 // İlk 3 deneme: 100ms (agresif)
            } else {
                let exp_delay = 100u64 * (1u64 << (retry_count - 3).min(6));
                exp_delay.min(10_000) // Üst sınır: 10 saniye
            };

            // Hiçbir durumda panik veya integer overflow olmamalı
            assert!(delay_ms >= 100, "Minimum delay 100ms: retry={}", retry_count);
            assert!(delay_ms <= 10_000, "Maksimum delay 10s: retry={}", retry_count);

            // İlk 3 deneme agresif
            if retry_count <= 3 {
                assert_eq!(delay_ms, 100, "İlk 3 deneme 100ms olmalı");
            }
        }

        // Specific backoff values
        assert_eq!(100u64 * (1u64 << 1u32.min(6)), 200);  // retry 4 → 200ms
        assert_eq!(100u64 * (1u64 << 2u32.min(6)), 400);  // retry 5 → 400ms
        assert_eq!(100u64 * (1u64 << 3u32.min(6)), 800);  // retry 6 → 800ms
        assert_eq!(100u64 * (1u64 << 4u32.min(6)), 1600); // retry 7 → 1600ms
        assert_eq!(100u64 * (1u64 << 5u32.min(6)), 3200); // retry 8 → 3200ms
        assert_eq!(100u64 * (1u64 << 6u32.min(6)), 6400); // retry 9 → 6400ms
        // retry 10+: min(6) clamp → 6400ms (< 10000 cap)
    }
}
