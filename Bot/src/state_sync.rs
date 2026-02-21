// ============================================================================
//  STATE_SYNC v7.0 — Paralel RPC + TickBitmap Derinlik Okuma
//
//  v7.0 Yenilikler:
//  ✓ futures::future::join_all ile tüm RPC çağrıları paralel
//  ✓ sync_tick_bitmap: word okuma + tick detay okuma paralel
//  ✓ sync_all_pools, sync_all_tick_bitmaps, cache_all_bytecodes paralel
//  ✓ read_storage_slots paralel
//
//  Önceki sıralı for-await döngüleri → join_all ile N'li paralel batch.
//  10 word + 30 tick = eskiden 40 sıralı RPC, şimdi 2 paralel batch.
// ============================================================================

use alloy::primitives::{Address, U256};
use alloy::providers::Provider;
use alloy::transports::Transport;
use alloy::network::Ethereum;
use alloy::sol;
use eyre::Result;
use std::time::Instant;
use futures_util::future::join_all;

use crate::math::compute_eth_price;
use crate::types::{DexType, PoolConfig, SharedPoolState, TickBitmapData, TickInfo};

// ─────────────────────────────────────────────────────────────────────────────
// Uniswap V3 Havuz Arayüzü (slot0 → 7 değişken, feeProtocol DAHİL)
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
// Tek Havuz Durum Senkronizasyonu
// ─────────────────────────────────────────────────────────────────────────────

/// Tek bir havuzun durumunu RPC üzerinden oku ve SharedPoolState'e yaz
pub async fn sync_pool_state<T: Transport + Clone, P: Provider<T, Ethereum> + Sync>(
    provider: &P,
    pool_config: &PoolConfig,
    pool_state: &SharedPoolState,
    block_number: u64,
) -> Result<()> {
    let (sqrt_price_x96, tick, liquidity) = match pool_config.dex {
        DexType::UniswapV3 => {
            let pool = IUniswapV3Pool::new(pool_config.address, provider);
            let slot0 = pool.slot0().call().await
                .map_err(|e| eyre::eyre!("[{}] slot0 okuma hatası (V3/7-alan): {}", pool_config.name, e))?;
            let liq = pool.liquidity().call().await
                .map_err(|e| eyre::eyre!("[{}] liquidity okuma hatası: {}", pool_config.name, e))?;
            (slot0.sqrtPriceX96, slot0.tick, liq._0)
        }
        DexType::Aerodrome => {
            let pool = IAerodromePool::new(pool_config.address, provider);
            let slot0 = pool.slot0().call().await
                .map_err(|e| eyre::eyre!("[{}] slot0 okuma hatası (Aero/6-alan): {}", pool_config.name, e))?;
            let liq = pool.liquidity().call().await
                .map_err(|e| eyre::eyre!("[{}] liquidity okuma hatası: {}", pool_config.name, e))?;
            (slot0.sqrtPriceX96, slot0.tick, liq._0)
        }
    };

    let sqrt_price_f64: f64 = {
        let s = sqrt_price_x96.to_string();
        s.parse::<f64>().unwrap_or(0.0)
    };
    let liquidity_f64: f64 = liquidity.to_string().parse::<f64>().unwrap_or(0.0);

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
    }

    Ok(())
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

/// Havuzun TickBitmap'ini belirli bir aralıkta oku
///
/// Bu fonksiyon:
///   1. Mevcut tick etrafındaki word pozisyonlarını hesaplar
///   2. tickBitmap(wordPos) çağrılarıyla hangi tick'lerin başlatıldığını bulur
///   3. Başlatılmış tick'ler için ticks(tick) çağrısıyla detay bilgisi çeker
///   4. Tüm veriyi TickBitmapData yapısına paketler
///
/// Performans: ~10-30 RPC çağrısı (range'e bağlı), paralel hale getirilebilir
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

    // Tarama aralığı: current_tick ± (scan_range * tick_spacing)
    let tick_lo = current_tick - (scan_range as i32 * tick_spacing);
    let tick_hi = current_tick + (scan_range as i32 * tick_spacing);

    // Word pozisyon aralığı
    let word_lo = tick_to_word_pos(tick_lo, tick_spacing);
    let word_hi = tick_to_word_pos(tick_hi, tick_spacing);

    let mut bitmap_data = TickBitmapData::empty();
    bitmap_data.scan_range = scan_range;
    bitmap_data.snapshot_block = block_number;

    // ── Adım 1: tickBitmap word'lerini PARALEL oku ────────────
    let mut all_initialized_ticks: Vec<i32> = Vec::new();

    // Her word pozisyonu için bir future oluştur
    let word_futures: Vec<_> = (word_lo..=word_hi)
        .map(|word_pos| {
            let pool_addr = pool_config.address;
            let dex = pool_config.dex.clone();
            let ts = tick_spacing;
            async move {
                let word = match dex {
                    DexType::UniswapV3 => {
                        let pool = IUniswapV3Pool::new(pool_addr, provider);
                        pool.tickBitmap(word_pos).call().await
                            .map(|r| r._0)
                            .unwrap_or(U256::ZERO)
                    }
                    DexType::Aerodrome => {
                        let pool = IAerodromePool::new(pool_addr, provider);
                        pool.tickBitmap(word_pos).call().await
                            .map(|r| r._0)
                            .unwrap_or(U256::ZERO)
                    }
                };
                (word_pos, word, ts)
            }
        })
        .collect();

    // Tüm word okumalarını paralel çalıştır
    let word_results = join_all(word_futures).await;

    for (word_pos, word, ts) in word_results {
        if word != U256::ZERO {
            bitmap_data.words.insert(word_pos, word);
            let initialized = extract_initialized_bits(word, word_pos, ts);
            all_initialized_ticks.extend(initialized);
        }
    }

    // ── Adım 2: Başlatılmış tick'lerin detaylarını PARALEL oku ──
    // Tarama aralığındaki tick'leri filtrele
    all_initialized_ticks.retain(|t| *t >= tick_lo && *t <= tick_hi);

    // Her tick için bir future oluştur
    let tick_futures: Vec<_> = all_initialized_ticks
        .iter()
        .map(|&tick| {
            let pool_addr = pool_config.address;
            let dex = pool_config.dex.clone();
            async move {
                let tick_i24 = tick.clamp(-887272, 887272) as i32;
                let result = match dex {
                    DexType::UniswapV3 => {
                        let pool = IUniswapV3Pool::new(pool_addr, provider);
                        pool.ticks(tick_i24.try_into().unwrap_or(0)).call().await
                            .map(|r| (r.liquidityGross, r.liquidityNet, r.initialized))
                            .ok()
                    }
                    DexType::Aerodrome => {
                        let pool = IAerodromePool::new(pool_addr, provider);
                        pool.ticks(tick_i24.try_into().unwrap_or(0)).call().await
                            .map(|r| (r.liquidityGross, r.liquidityNet, r.initialized))
                            .ok()
                    }
                };
                (tick, result)
            }
        })
        .collect();

    // Tüm tick detay okumalarını paralel çalıştır
    let tick_results = join_all(tick_futures).await;

    for (tick, maybe_data) in tick_results {
        if let Some((liq_gross, liq_net, initialized)) = maybe_data {
            if initialized {
                bitmap_data.ticks.insert(tick, TickInfo {
                    liquidity_gross: liq_gross,
                    liquidity_net: liq_net,
                    initialized: true,
                });
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

pub async fn cache_pool_bytecode<T: Transport + Clone, P: Provider<T, Ethereum> + Sync>(
    provider: &P,
    pool_config: &PoolConfig,
    pool_state: &SharedPoolState,
) -> Result<()> {
    let code = provider
        .get_code_at(pool_config.address)
        .await
        .map_err(|e| eyre::eyre!("[{}] Bytecode okuma hatası: {}", pool_config.name, e))?;

    let mut state = pool_state.write();
    state.bytecode = Some(code.to_vec());

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Toplu Senkronizasyon
// ─────────────────────────────────────────────────────────────────────────────

pub async fn sync_all_pools<T: Transport + Clone, P: Provider<T, Ethereum> + Sync>(
    provider: &P,
    pools: &[PoolConfig],
    states: &[SharedPoolState],
    block_number: u64,
) -> Vec<Result<()>> {
    let futures: Vec<_> = pools.iter().zip(states.iter())
        .map(|(config, state)| sync_pool_state(provider, config, state, block_number))
        .collect();
    join_all(futures).await
}

/// Tüm havuzların TickBitmap'lerini senkronize et
///
/// Her havuz için:
///   1. tickBitmap word'lerini tarar
///   2. Başlatılmış tick'lerin liquidityNet bilgisini okur
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

// ─────────────────────────────────────────────────────────────────────────────
// Ek Depolama Yuvası Okuma (REVM Simülasyonu İçin)
// ─────────────────────────────────────────────────────────────────────────────

/// Belirli bir depolama yuvasını oku (REVM veritabanını doldurmak için)
#[allow(dead_code)]
pub async fn read_storage_slot<T: Transport + Clone, P: Provider<T, Ethereum> + Sync>(
    provider: &P,
    address: Address,
    slot: U256,
) -> Result<U256> {
    let value = provider
        .get_storage_at(address, slot)
        .await
        .map_err(|e| eyre::eyre!("Storage slot okuma hatası [{} @ slot {}]: {}", address, slot, e))?;

    Ok(value)
}

/// Birden fazla depolama yuvasını oku
#[allow(dead_code)]
pub async fn read_storage_slots<T: Transport + Clone, P: Provider<T, Ethereum> + Sync>(
    provider: &P,
    address: Address,
    slots: &[U256],
) -> Vec<Result<U256>> {
    let futures: Vec<_> = slots.iter()
        .map(|&slot| read_storage_slot(provider, address, slot))
        .collect();
    join_all(futures).await
}
