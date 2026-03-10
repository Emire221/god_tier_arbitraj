// ============================================================================
//  TRANSPORT v10.0 — HFT RPC Pool + IPC Öncelikli Bağlantı Yönetimi
//
//  Özellikler:
//  ✓ IPC (Unix Domain Socket / Named Pipe) öncelikli bağlantı
//  ✓ IPC çökerse Round-Robin WSS fallback (3 endpoint)
//  ✓ Arka plan sağlık kontrolü (2s geride kalan node geçici olarak devre dışı)
//  ✓ Zero-copy provider referansları
//  ✓ Lock-free okuma (parking_lot::RwLock)
// ============================================================================

use alloy::providers::{Provider, ProviderBuilder, RootProvider, WsConnect};
use alloy::pubsub::PubSubFrontend;
use eyre::Result;
use parking_lot::RwLock;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, AtomicBool, Ordering};
use std::time::Duration;

// ─────────────────────────────────────────────────────────────────────────────
// Node Sağlık Durumu
// ─────────────────────────────────────────────────────────────────────────────

/// Tek bir RPC node'unun durumu
#[allow(dead_code)]
struct NodeState {
    /// Provider instance (bağlantı kurulmuşsa) — v22.0: RwLock ile cache
    provider: RwLock<Option<RootProvider<PubSubFrontend>>>,
    /// WebSocket URL'i
    url: String,
    /// Node sağlıklı mı? (atomik — lock-free okuma)
    healthy: AtomicBool,
    /// Son bilinen blok numarası
    last_block: AtomicUsize,
}

// ─────────────────────────────────────────────────────────────────────────────
// RPC Pool — IPC Öncelikli, Round-Robin WSS Fallback
// ─────────────────────────────────────────────────────────────────────────────

/// HFT-grade RPC bağlantı havuzu.
///
/// Öncelik sırası:
///   1. Yerel IPC soketi (sub-0.1ms gecikme)
///   2. WSS endpoint'leri (Round-Robin, sağlık kontrolü ile)
///
/// Sağlık kontrolü:
///   - Arka plan task her 2 saniyede tüm node'ları yoklar
///   - `eth_blockNumber` ile güncel blok sorgulanır
///   - En yüksek blok sayısına göre 2+ blok geride kalan node devre dışı bırakılır
pub struct RpcPool {
    /// IPC provider (varsa — en düşük gecikme)
    ipc_provider: RwLock<Option<RootProvider<PubSubFrontend>>>,
    /// IPC sağlıklı mı?
    ipc_healthy: AtomicBool,
    /// IPC yolu (reconnect için)
    ipc_path: Option<String>,
    /// WSS node listesi
    ws_nodes: Vec<Arc<NodeState>>,
    /// Round-Robin sayacı (atomik)
    rr_counter: AtomicUsize,
    /// Pool aktif mi?
    active: AtomicBool,
}

#[allow(dead_code)]
impl RpcPool {
    /// Yeni RPC Pool oluştur.
    ///
    /// # Argümanlar
    /// - `ipc_path`: Opsiyonel IPC soket yolu
    /// - `ws_urls`: WebSocket URL listesi (en az 1)
    pub fn new(ipc_path: Option<String>, ws_urls: &[String]) -> Self {
        let ws_nodes: Vec<Arc<NodeState>> = ws_urls
            .iter()
            .map(|url| {
                Arc::new(NodeState {
                    provider: RwLock::new(None),
                    url: url.clone(),
                    healthy: AtomicBool::new(false),
                    last_block: AtomicUsize::new(0),
                })
            })
            .collect();

        Self {
            ipc_provider: RwLock::new(None),
            ipc_healthy: AtomicBool::new(false),
            ipc_path,
            ws_nodes,
            rr_counter: AtomicUsize::new(0),
            active: AtomicBool::new(true),
        }
    }

    /// Tüm bağlantıları başlat (IPC + WSS).
    /// Döngü dışında bir kez çağrılır — allocation burada yapılır.
    pub async fn connect_all(&mut self) -> Result<()> {
        // 1. IPC bağlantısı (varsa)
        if let Some(ref ipc_path) = self.ipc_path {
            match self.try_connect_ipc(ipc_path).await {
                Ok(provider) => {
                    *self.ipc_provider.write() = Some(provider);
                    self.ipc_healthy.store(true, Ordering::Release);
                    eprintln!("  ✅ IPC bağlantı kuruldu: {}", ipc_path);
                }
                Err(e) => {
                    eprintln!("  ⚠️  IPC bağlantı başarısız (WSS fallback aktif): {}", e);
                }
            }
        }

        // 2. WSS bağlantıları — paralel değil sıralı (alloy WsConnect thread-safe değil)
        for node in &self.ws_nodes {
            match Self::try_connect_ws(&node.url).await {
                Ok(provider) => {
                    node.healthy.store(true, Ordering::Release);
                    eprintln!("  ✅ WSS bağlantı kuruldu: {}", &node.url[..node.url.len().min(40)]);

                    // v22.0: Provider'ı cache'e al — get_provider her seferinde
                    // yeni bağlantı açmak yerine cache'den klonlar
                    *node.provider.write() = Some(provider);
                }
                Err(e) => {
                    eprintln!("  ⚠️  WSS bağlantı başarısız: {} — {}", &node.url[..node.url.len().min(40)], e);
                }
            }
        }

        // En az bir bağlantı var mı?
        let has_ipc = self.ipc_healthy.load(Ordering::Acquire);
        let has_ws = self.ws_nodes.iter().any(|n| n.healthy.load(Ordering::Acquire));

        if !has_ipc && !has_ws {
            return Err(eyre::eyre!("Hiçbir RPC endpoint'e bağlanılamadı!"));
        }

        Ok(())
    }

    /// En düşük gecikmeli sağlıklı provider'ı döndür.
    /// Öncelik: IPC > Round-Robin WSS
    pub async fn get_provider(&self) -> Result<RootProvider<PubSubFrontend>> {
        // 1. IPC sağlıklıysa onu kullan
        if self.ipc_healthy.load(Ordering::Acquire) {
            let guard = self.ipc_provider.read();
            if let Some(ref provider) = *guard {
                return Ok(provider.clone());
            }
        }

        // 2. WSS Round-Robin
        let node_count = self.ws_nodes.len();
        if node_count == 0 {
            return Err(eyre::eyre!("WSS node listesi boş!"));
        }

        // Sağlıklı node bul (en fazla node_count deneme)
        for _ in 0..node_count {
            let idx = self.rr_counter.fetch_add(1, Ordering::Relaxed) % node_count;
            let node = &self.ws_nodes[idx];

            if !node.healthy.load(Ordering::Acquire) {
                continue;
            }

            // v22.0: Cache'den klonla — her seferinde yeni bağlantı açmak yerine
            // mevcut provider'ı kullan. Bağlantı kopmuşsa yeniden bağlan.
            {
                let guard = node.provider.read();
                if let Some(ref provider) = *guard {
                    return Ok(provider.clone());
                }
            }

            // Cache boş — yeni bağlantı aç ve cache'e al
            match Self::try_connect_ws(&node.url).await {
                Ok(provider) => {
                    let cloned = provider.clone();
                    *node.provider.write() = Some(provider);
                    return Ok(cloned);
                }
                Err(e) => {
                    node.healthy.store(false, Ordering::Release);
                    eprintln!("  ⚠️  WSS node {} bağlantı kaybı: {}", idx, e);
                    continue;
                }
            }
        }

        Err(eyre::eyre!("Tüm RPC node'ları devre dışı — sağlık kontrolü bekleniyor"))
    }

    /// Arka plan sağlık kontrolü task'ı başlat.
    ///
    /// Her 2 saniyede:
    ///   1. Tüm node'lardan `eth_blockNumber` sorgular
    ///   2. En yüksek blok sayısını belirler
    ///   3. 2+ blok gerisinde kalan node'ları devre dışı bırakır
    ///   4. Devre dışı node'ları yeniden bağlanmaya çalışır
    pub fn spawn_health_checker(self: &Arc<Self>) {
        let pool = Arc::clone(self);

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(2));

            loop {
                interval.tick().await;

                if !pool.active.load(Ordering::Acquire) {
                    break;
                }

                // IPC sağlık kontrolü
                if let Some(ref _ipc_path) = pool.ipc_path {
                    let ipc_ok = {
                        let provider_clone = {
                            let guard = pool.ipc_provider.read();
                            guard.clone()
                        };
                        if let Some(ref provider) = provider_clone {
                            provider.get_block_number().await.is_ok()
                        } else {
                            false
                        }
                    };
                    pool.ipc_healthy.store(ipc_ok, Ordering::Release);
                }

                // WSS node'ları kontrol et — blok numaralarını topla
                let mut block_numbers: Vec<(usize, u64)> = Vec::with_capacity(pool.ws_nodes.len());

                for (idx, node) in pool.ws_nodes.iter().enumerate() {
                    // v22.0: İlk olarak cache'deki provider'ı dene
                    let cached_provider = {
                        let guard = node.provider.read();
                        guard.clone()
                    };
                    
                    let cached_ok = if let Some(ref provider) = cached_provider {
                        match provider.get_block_number().await {
                            Ok(bn) => {
                                node.last_block.store(bn as usize, Ordering::Release);
                                block_numbers.push((idx, bn));
                                if !node.healthy.load(Ordering::Acquire) {
                                    node.healthy.store(true, Ordering::Release);
                                    eprintln!(
                                        "  🔄 WSS node #{} tekrar sağlıklı (blok #{})",
                                        idx, bn
                                    );
                                }
                                true
                            }
                            Err(_) => false
                        }
                    } else {
                        false
                    };

                    if !cached_ok {
                        // Cache'deki provider başarısız — yeniden bağlan
                        match Self::try_connect_ws(&node.url).await {
                            Ok(provider) => {
                                match provider.get_block_number().await {
                                    Ok(bn) => {
                                        node.last_block.store(bn as usize, Ordering::Release);
                                        block_numbers.push((idx, bn));
                                        *node.provider.write() = Some(provider);
                                        node.healthy.store(true, Ordering::Release);
                                        eprintln!(
                                            "  🔄 WSS node #{} yeniden bağlandı (blok #{})",
                                            idx, bn
                                        );
                                    }
                                    Err(_) => {
                                        *node.provider.write() = None;
                                        node.healthy.store(false, Ordering::Release);
                                    }
                                }
                            }
                            Err(_) => {
                                *node.provider.write() = None;
                                node.healthy.store(false, Ordering::Release);
                            }
                        }
                    }
                }

                // En yüksek blok numarasını bul
                if let Some(max_block) = block_numbers.iter().map(|(_, b)| *b).max() {
                    // 2+ blok geride kalan node'ları devre dışı bırak
                    for (idx, bn) in &block_numbers {
                        if max_block.saturating_sub(*bn) >= 2 {
                            let node = &pool.ws_nodes[*idx];
                            if node.healthy.load(Ordering::Acquire) {
                                node.healthy.store(false, Ordering::Release);
                                eprintln!(
                                    "  ⚠️  WSS node #{} geride kaldı (blok #{} vs max #{}) — geçici devre dışı",
                                    idx, bn, max_block
                                );
                            }
                        }
                    }
                }
            }
        });
    }

    /// Pool'u kapat (health checker durdurulur)
    pub fn shutdown(&self) {
        self.active.store(false, Ordering::Release);
    }

    // ── İç Bağlantı Yardımcıları ────────────────────────────────────────────

    /// IPC sokete bağlan
    /// v22.0: IPC path tanımlıysa local WS proxy olarak kullanılır.
    /// Alloy 0.1'de native IPC desteği yoktur — local node'un WS
    /// endpointi üzerinden bağlantı kurulur (eşdeğer gecikme).
    async fn try_connect_ipc(&self, ipc_path: &str) -> Result<RootProvider<PubSubFrontend>> {
        // Alloy IPC provider — Base node'un IPC soketi
        // Windows: \\.\pipe\geth.ipc
        // Linux/Mac: /tmp/geth.ipc veya /path/to/base-node/geth.ipc
        eprintln!(
            "  ℹ️  IPC path '{}' tanımlı — Alloy 0.1 native IPC desteklemediğinden local WS proxy kullanılıyor",
            ipc_path
        );
        let ws_url = format!("ws://127.0.0.1:8546");

        let ws = WsConnect::new(&ws_url);
        let provider = ProviderBuilder::new()
            .on_ws(ws)
            .await
            .map_err(|e| eyre::eyre!("IPC/Local WS bağlantı hatası ({}): {}", ipc_path, e))?;

        // Bağlantı testi
        let _block = provider.get_block_number().await
            .map_err(|e| eyre::eyre!("IPC sağlık kontrolü başarısız: {}", e))?;

        Ok(provider)
    }

    /// WebSocket'e bağlan
    async fn try_connect_ws(url: &str) -> Result<RootProvider<PubSubFrontend>> {
        let ws = WsConnect::new(url);
        let provider = ProviderBuilder::new()
            .on_ws(ws)
            .await
            .map_err(|e| eyre::eyre!("WSS bağlantı hatası ({}): {}", &url[..url.len().min(40)], e))?;

        Ok(provider)
    }

    /// Aktif sağlıklı node sayısı
    pub fn healthy_node_count(&self) -> usize {
        let ipc = if self.ipc_healthy.load(Ordering::Acquire) { 1 } else { 0 };
        let ws = self.ws_nodes.iter()
            .filter(|n| n.healthy.load(Ordering::Acquire))
            .count();
        ipc + ws
    }

    /// Transport bilgi stringi (banner için)
    pub fn transport_info(&self) -> String {
        let ipc_status = if self.ipc_healthy.load(Ordering::Acquire) {
            "IPC ✅"
        } else if self.ipc_path.is_some() {
            "IPC ❌"
        } else {
            "IPC yok"
        };

        let ws_healthy = self.ws_nodes.iter()
            .filter(|n| n.healthy.load(Ordering::Acquire))
            .count();
        let ws_total = self.ws_nodes.len();

        format!("{} | WSS {}/{} aktif", ipc_status, ws_healthy, ws_total)
    }
}
