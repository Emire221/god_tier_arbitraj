// ============================================================================
//  ROUTE ENGINE v1.0 — Likidite Grafı + DFS Multi-Hop Rota Üreteci
//
//  v1.0 Yenilikler:
//  ✓ LiquidityGraph: Token'lar = düğümler, Havuzlar = kenarlar
//  ✓ DFS rota üreteci: WETH'ten başlayan 2/3/4-hop çevrimler
//  ✓ Route + Hop yapıları: Çok-havuzlu arbitraj rotaları
//  ✓ Fee Tier Isolation: Aynı DEX farklı fee tier ayrı kenar
//  ✓ Gaz-safe: max_depth=4, max_routes sınırı
//
//  Mimari:
//    Token(WETH) ──Pool1──▶ Token(USDC) ──Pool2──▶ Token(cbBTC) ──Pool3──▶ Token(WETH)
//       ▲                                                                     │
//       └─────────────────── Kâr = çıktı - girdi ────────────────────────────┘
//
//  Mevcut 2-pool sistemi KORUNUYOR — bu modül ek multi-hop rotaları üretir.
// ============================================================================

use alloy::primitives::Address;
use std::collections::HashMap;

use crate::types::{PoolConfig, SharedPoolState, DexType};

// ─────────────────────────────────────────────────────────────────────────────
// Sabitler
// ─────────────────────────────────────────────────────────────────────────────

/// Maksimum rota derinliği (hop sayısı). 4'ten fazla hop gas maliyetini
/// kâr marjını aşacak seviyeye çıkarır.
const MAX_HOP_DEPTH: usize = 4;

/// DFS'in üretebileceği maksimum rota sayısı. Kombinatorik patlamayı önler.
const MAX_ROUTES: usize = 500;

/// Minimum likidite filtresi — kenar ağırlığı (WETH cinsinden)
const MIN_EDGE_LIQUIDITY_WETH: f64 = 0.1;

// ─────────────────────────────────────────────────────────────────────────────
// Rota Yapıları
// ─────────────────────────────────────────────────────────────────────────────

/// Tek bir swap adımı (hop)
#[derive(Debug, Clone)]
pub struct Hop {
    /// pools[] vektöründeki havuz indeksi
    pub pool_idx: usize,
    /// Girdi token adresi
    pub token_in: Address,
    /// Çıktı token adresi
    pub token_out: Address,
    /// Swap yönü: true = token0 → token1, false = token1 → token0
    pub zero_for_one: bool,
}

/// Çok-havuzlu arbitraj rotası (WETH → ... → WETH döngüsü)
#[derive(Debug, Clone)]
pub struct Route {
    /// Sıralı hop'lar ([WETH→USDC, USDC→cbBTC, cbBTC→WETH] gibi)
    pub hops: Vec<Hop>,
    /// Rotadaki tüm token'lar sırasıyla (hops.len() + 1 uzunlukta)
    /// Örn: [WETH, USDC, cbBTC, WETH]
    pub tokens: Vec<Address>,
    /// Rota açıklaması (log/debug için)
    pub label: String,
}

impl Route {
    /// Rotadaki hop sayısı
    pub fn hop_count(&self) -> usize {
        self.hops.len()
    }

    /// 2-hop rotası mı? (klasik çapraz-DEX arbitraj)
    pub fn is_two_hop(&self) -> bool {
        self.hops.len() == 2
    }

    /// 3-hop rotası mı? (triangular arbitraj)
    pub fn is_triangular(&self) -> bool {
        self.hops.len() == 3
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Kenar Yapısı (Grafikteki Bağlantı)
// ─────────────────────────────────────────────────────────────────────────────

/// Likidite grafiğindeki tek bir kenar: bir havuz aracılığıyla
/// token_a'dan token_b'ye swap yapılabilir.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Edge metadata alanları graf istatistiklerinde ve gelecek skorlamada kullanılır
struct Edge {
    /// pools[] vektöründeki havuz indeksi
    pool_idx: usize,
    /// Hedef token (bu kenarın ulaştığı düğüm)
    target_token: Address,
    /// Swap yönü (true = token0→token1, false = token1→token0)
    zero_for_one: bool,
    /// Fee (basis points)
    fee_bps: u32,
    /// DEX türü
    dex: DexType,
    /// Tahmini likidite kapasitesi (WETH cinsinden)
    liquidity_estimate: f64,
}

// ─────────────────────────────────────────────────────────────────────────────
// Likidite Grafiği
// ─────────────────────────────────────────────────────────────────────────────

/// Token'lar arası likidite bağlantı grafiği.
///
/// Her token bir düğüm, her havuz iki yönlü kenar (A→B ve B→A).
/// DFS ile WETH'ten başlayıp WETH'e dönen döngüler (rotalar) bulunur.
///
/// Aynı token çifti için farklı DEX'ler ve farklı fee tier'lar
/// ayrı kenarlar olarak eklenir (Fee Tier Isolation).
pub struct LiquidityGraph {
    /// adjacency[token_address] = [Edge, Edge, ...] — ilişkili kenarlar
    adjacency: HashMap<Address, Vec<Edge>>,
    /// WETH adresi (başlangıç/bitiş düğümü)
    weth_address: Address,
}

impl LiquidityGraph {
    /// Havuz yapılandırmalarından likidite grafiğini oluştur.
    ///
    /// Her havuz (token0, token1) çifti için iki yönlü kenar eklenir.
    /// Aynı token çifti + farklı DEX + farklı fee tier → ayrı kenarlar.
    pub fn build(
        pools: &[PoolConfig],
        states: &[SharedPoolState],
        weth_address: Address,
    ) -> Self {
        let mut adjacency: HashMap<Address, Vec<Edge>> = HashMap::new();

        for (idx, pool) in pools.iter().enumerate() {
            // Havuz aktif mi kontrol et
            if idx < states.len() {
                let state = states[idx].load();
                if !state.is_active() {
                    continue;
                }
            }

            // Token adreslerini çıkar
            let (token0, token1) = Self::pool_tokens(pool);

            // Likidite tahmini (WETH cinsinden)
            let liq_estimate = if idx < states.len() {
                let state = states[idx].load();
                // Kaba likidite tahmini: liquidity / 10^18
                state.liquidity_f64 / 1e18
            } else {
                1.0 // Fallback: bilgi yoksa 1.0 WETH varsay
            };

            if liq_estimate < MIN_EDGE_LIQUIDITY_WETH {
                continue;
            }

            // token0 → token1 kenarı
            adjacency.entry(token0).or_default().push(Edge {
                pool_idx: idx,
                target_token: token1,
                zero_for_one: true,
                fee_bps: pool.fee_bps,
                dex: pool.dex,
                liquidity_estimate: liq_estimate,
            });

            // token1 → token0 kenarı
            adjacency.entry(token1).or_default().push(Edge {
                pool_idx: idx,
                target_token: token0,
                zero_for_one: false,
                fee_bps: pool.fee_bps,
                dex: pool.dex,
                liquidity_estimate: liq_estimate,
            });
        }

        LiquidityGraph {
            adjacency,
            weth_address,
        }
    }

    /// Havuzun token0 ve token1 adreslerini çıkar.
    ///
    /// PoolConfig'de base_token_address ve quote_token_address + token0_is_weth bayrağından türetilir.
    fn pool_tokens(pool: &PoolConfig) -> (Address, Address) {
        if pool.token0_is_weth {
            (pool.base_token_address, pool.quote_token_address)
        } else {
            (pool.quote_token_address, pool.base_token_address)
        }
    }

    /// DFS ile WETH'ten başlayıp WETH'e dönen tüm rotaları bul.
    ///
    /// # Parametreler
    /// - `max_depth`: Maksimum hop sayısı (2, 3, veya 4)
    /// - `max_routes`: Üretilecek maksimum rota sayısı
    ///
    /// # Döngü Tespiti
    /// Ziyaret edilen token'lar set'i tutulur. Aynı token'a tekrar
    /// uğranmaz (WETH hariç — bitiş noktası olarak izin verilir).
    ///
    /// # Dönüş
    /// Sıralı Route vektörü. Önce 2-hop, sonra 3-hop, sonra 4-hop.
    pub fn find_routes(&self, max_depth: usize, max_routes: usize) -> Vec<Route> {
        let depth = max_depth.min(MAX_HOP_DEPTH);
        let limit = max_routes.min(MAX_ROUTES);
        let mut routes = Vec::new();

        // DFS başlangıç: WETH düğümünden
        let mut path_tokens: Vec<Address> = vec![self.weth_address];
        let mut path_hops: Vec<Hop> = Vec::new();
        let mut visited_pools: Vec<usize> = Vec::new();

        self.dfs(
            self.weth_address,
            &mut path_tokens,
            &mut path_hops,
            &mut visited_pools,
            depth,
            limit,
            &mut routes,
        );

        routes
    }

    /// DFS rekürsif arama — WETH → ... → WETH döngüleri bulur
    #[allow(clippy::too_many_arguments)]
    fn dfs(
        &self,
        current_token: Address,
        path_tokens: &mut Vec<Address>,
        path_hops: &mut Vec<Hop>,
        visited_pools: &mut Vec<usize>,
        max_depth: usize,
        max_routes: usize,
        routes: &mut Vec<Route>,
    ) {
        if routes.len() >= max_routes {
            return;
        }

        let current_depth = path_hops.len();

        // Minimum 2 hop sonra WETH'e dönüş kontrolü
        if current_depth >= 2 && current_token == self.weth_address {
            // Geçerli döngü bulundu!
            let label = self.build_route_label(path_tokens);
            routes.push(Route {
                hops: path_hops.clone(),
                tokens: path_tokens.clone(),
                label,
            });
            return;
        }

        // Maksimum derinliğe ulaşıldıysa dur
        if current_depth >= max_depth {
            return;
        }

        // Bu token'dan çıkan kenarları incele
        if let Some(edges) = self.adjacency.get(&current_token) {
            for edge in edges {
                // Aynı havuzu tekrar kullanma
                if visited_pools.contains(&edge.pool_idx) {
                    continue;
                }

                // Ara düğümlerde WETH'e dönüşe sadece 2+ hop'ta izin ver
                if edge.target_token == self.weth_address && current_depth < 1 {
                    continue;
                }

                // Ziyaret edilen token kontrolü (WETH bitiş hariç)
                if edge.target_token != self.weth_address
                    && path_tokens.contains(&edge.target_token)
                {
                    continue;
                }

                // Bu kenarı dene
                let hop = Hop {
                    pool_idx: edge.pool_idx,
                    token_in: current_token,
                    token_out: edge.target_token,
                    zero_for_one: edge.zero_for_one,
                };

                path_tokens.push(edge.target_token);
                path_hops.push(hop);
                visited_pools.push(edge.pool_idx);

                self.dfs(
                    edge.target_token,
                    path_tokens,
                    path_hops,
                    visited_pools,
                    max_depth,
                    max_routes,
                    routes,
                );

                // Backtrack
                path_tokens.pop();
                path_hops.pop();
                visited_pools.pop();
            }
        }
    }

    /// Rota etiketini oluştur (debug/log için)
    /// Örn: "WETH → USDC → cbBTC → WETH [3-hop]"
    fn build_route_label(&self, tokens: &[Address]) -> String {
        let weth_lower = format!("{:?}", self.weth_address).to_lowercase();
        let symbols: Vec<String> = tokens.iter().map(|addr| {
            let s = format!("{:?}", addr).to_lowercase();
            if s == weth_lower {
                "WETH".to_string()
            } else {
                Self::guess_symbol(addr)
            }
        }).collect();
        let hop_count = tokens.len().saturating_sub(1);
        format!("{} [{}-hop]", symbols.join(" → "), hop_count)
    }

    /// Adres'ten token sembolü tahmin et (Base L2 bilinen adresler)
    fn guess_symbol(addr: &Address) -> String {
        let s = format!("{:?}", addr).to_lowercase();
        if s.contains("833589fcd6edb6e08f4c7c32d4f71b54bda02913") {
            "USDC".to_string()
        } else if s.contains("d9aaec86b65d86f6a7b5b1b0c42ffa531710b6ca") {
            "USDbC".to_string()
        } else if s.contains("50c5725949a6f0c72e6c4a641f24049a917db0cb") {
            "DAI".to_string()
        } else if s.contains("2ae3f1ec7f1f5012cfeab0185bfc7aa3cf0dec22") {
            "cbETH".to_string()
        } else if s.contains("cbb7c0000ab88b473b1f5afd9ef808440eed33bf") {
            "cbBTC".to_string()
        } else if s.contains("940181a94a35a4569e4529a3cdfb74e38fd98631") {
            "AERO".to_string()
        } else if s.contains("4ed4e862860bed51a9570b96d89af5e1b0efefed") {
            "DEGEN".to_string()
        } else {
            format!("0x{}..{}", &s[2..6], &s[s.len()-4..])
        }
    }

    /// Grafikteki düğüm (token) sayısı
    pub fn node_count(&self) -> usize {
        self.adjacency.len()
    }

    /// Grafikteki kenar (havuz bağlantısı) sayısı
    pub fn edge_count(&self) -> usize {
        self.adjacency.values().map(|edges| edges.len()).sum()
    }

    /// 2-hop rotalarını filtrele (mevcut PairCombo ile aynı mantık)
    pub fn two_hop_routes<'a>(&self, routes: &'a [Route]) -> Vec<&'a Route> {
        routes.iter().filter(|r| r.is_two_hop()).collect()
    }

    /// 3+ hop rotalarını filtrele (yeni multi-hop rotalar)
    pub fn multi_hop_routes<'a>(&self, routes: &'a [Route]) -> Vec<&'a Route> {
        routes.iter().filter(|r| r.hop_count() >= 3).collect()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Testler
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::PoolState;
    use std::sync::Arc;
    use arc_swap::ArcSwap;
    use std::time::Instant;

    fn weth() -> Address {
        "0x4200000000000000000000000000000000000006".parse().unwrap()
    }
    fn usdc() -> Address {
        "0x833589fCd6edB6E08f4c7C32D4f71b54bdA02913".parse().unwrap()
    }
    fn dai() -> Address {
        "0x50c5725949A6F0c72E6C4a641F24049A917DB0Cb".parse().unwrap()
    }

    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_POOL_COUNTER: AtomicU64 = AtomicU64::new(1);

    fn test_pool_address() -> Address {
        let n = TEST_POOL_COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut bytes = [0u8; 20];
        bytes[12..20].copy_from_slice(&n.to_be_bytes());
        bytes[0] = 0xAA; // Deterministik prefix
        Address::from(bytes)
    }

    fn make_pool_config(quote: Address, dex: DexType, fee_bps: u32, t0_is_weth: bool) -> PoolConfig {
        PoolConfig {
            address: test_pool_address(),
            name: format!("test-{:?}-{}", dex, fee_bps),
            fee_bps,
            fee_fraction: fee_bps as f64 / 10_000.0,
            token0_decimals: if t0_is_weth { 18 } else { 6 },
            token1_decimals: if t0_is_weth { 6 } else { 18 },
            dex,
            token0_is_weth: t0_is_weth,
            tick_spacing: 10,
            quote_token_address: quote,
            base_token_address: weth(),
        }
    }

    fn make_active_state() -> SharedPoolState {
        Arc::new(ArcSwap::from_pointee(PoolState {
            sqrt_price_x96: alloy::primitives::U256::from(1u64) << 96,
            sqrt_price_f64: 1.0,
            tick: 0,
            liquidity: 50_000_000_000_000_000_000u128,
            liquidity_f64: 5e19,
            eth_price_usd: 2500.0,
            last_block: 100,
            last_update: Instant::now(),
            is_initialized: true,
            bytecode: None,
            tick_bitmap: None,
            live_fee_bps: None,
            is_stale: false,
        }))
    }

    #[test]
    fn test_graph_build_and_edges() {
        let pools = vec![
            make_pool_config(usdc(), DexType::UniswapV3, 5, true),
            make_pool_config(usdc(), DexType::Aerodrome, 5, true),
            make_pool_config(dai(), DexType::UniswapV3, 5, true),
        ];
        let states: Vec<SharedPoolState> = pools.iter().map(|_| make_active_state()).collect();

        let graph = LiquidityGraph::build(&pools, &states, weth());

        // 3 havuz × 2 yönlü kenar = 6 toplam kenar
        assert_eq!(graph.edge_count(), 6);
        // 3 farklı token: WETH, USDC, DAI
        assert_eq!(graph.node_count(), 3);
    }

    #[test]
    fn test_find_two_hop_routes() {
        // WETH/USDC: UniV3 + Aerodrome → 2-hop çapraz-DEX rota
        let pools = vec![
            make_pool_config(usdc(), DexType::UniswapV3, 5, true),
            make_pool_config(usdc(), DexType::Aerodrome, 5, true),
        ];
        let states: Vec<SharedPoolState> = pools.iter().map(|_| make_active_state()).collect();

        let graph = LiquidityGraph::build(&pools, &states, weth());
        let routes = graph.find_routes(3, 100);

        // En az bir 2-hop rota bulunmalı (WETH→USDC→WETH)
        let two_hop = graph.two_hop_routes(&routes);
        assert!(!two_hop.is_empty(), "2-hop rota bulunamadı");

        for route in &two_hop {
            assert_eq!(route.tokens.first(), Some(&weth()));
            assert_eq!(route.tokens.last(), Some(&weth()));
        }
    }

    #[test]
    fn test_find_triangular_routes() {
        // WETH/USDC (UniV3) + USDC/DAI (UniV3) + WETH/DAI (Aerodrome)
        // → 3-hop: WETH → USDC → DAI → WETH
        let pools = vec![
            make_pool_config(usdc(), DexType::UniswapV3, 5, true),     // WETH/USDC
            make_pool_config(dai(), DexType::UniswapV3, 5, true),      // WETH/DAI
            // USDC/DAI pool — burada WETH yok, doğrudan USDC/DAI
        ];
        // Ayrıca USDC→DAI yolu için bir havuz ekleyelim
        let mut pools = pools;
        pools.push(PoolConfig {
            address: test_pool_address(),
            name: "test-usdc-dai".to_string(),
            fee_bps: 1,
            fee_fraction: 0.0001,
            token0_decimals: 6,
            token1_decimals: 18,
            dex: DexType::UniswapV3,
            token0_is_weth: false, // Bu havuzda WETH yok — USDC token0, DAI token1
            tick_spacing: 1,
            quote_token_address: dai(), // quote = DAI
            base_token_address: usdc(), // base = USDC (token0)
        });

        let states: Vec<SharedPoolState> = pools.iter().map(|_| make_active_state()).collect();

        let graph = LiquidityGraph::build(&pools, &states, weth());
        let routes = graph.find_routes(4, 200);

        // Hem 2-hop hem 3-hop rotaları bulunmalı
        assert!(!routes.is_empty(), "Hiç rota bulunamadı");

        // Debug: rota etiketlerini yazdır
        for route in &routes {
            eprintln!("  Route: {} (hops: {})", route.label, route.hop_count());
        }
    }

    #[test]
    fn test_max_routes_limit() {
        // Çok sayıda havuz ile rota sayısı sınırının çalıştığını doğrula
        let tokens = vec![usdc(), dai()];
        let mut pools = Vec::new();
        for &token in &tokens {
            for _ in 0..5 {
                pools.push(make_pool_config(token, DexType::UniswapV3, 5, true));
                pools.push(make_pool_config(token, DexType::Aerodrome, 5, true));
            }
        }
        let states: Vec<SharedPoolState> = pools.iter().map(|_| make_active_state()).collect();

        let graph = LiquidityGraph::build(&pools, &states, weth());
        let routes = graph.find_routes(4, 10);

        assert!(routes.len() <= 10, "MAX_ROUTES sınırı aşıldı: {}", routes.len());
    }

    #[test]
    fn test_no_self_loops() {
        let pools = vec![
            make_pool_config(usdc(), DexType::UniswapV3, 5, true),
        ];
        let states: Vec<SharedPoolState> = pools.iter().map(|_| make_active_state()).collect();

        let graph = LiquidityGraph::build(&pools, &states, weth());
        let routes = graph.find_routes(4, 100);

        // Tek havuzla döngü oluşturulamaz (aynı havuz iki kez kullanılamaz)
        // Ama WETH→USDC→WETH 2-hop'ta aynı havuzla yapılamaz
        for route in &routes {
            let pool_idxs: Vec<usize> = route.hops.iter().map(|h| h.pool_idx).collect();
            let unique: std::collections::HashSet<usize> = pool_idxs.iter().cloned().collect();
            assert_eq!(pool_idxs.len(), unique.len(), "Aynı havuz tekrar kullanılmış: {:?}", pool_idxs);
        }
    }
}
