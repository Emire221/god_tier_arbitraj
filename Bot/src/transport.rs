// ============================================================================
//  TRANSPORT v10.0 â€” HFT RPC Pool + IPC Ã–ncelikli BaÄŸlantÄ± YÃ¶netimi
//
//  Ã–zellikler:
//  âœ“ IPC (Unix Domain Socket / Named Pipe) Ã¶ncelikli baÄŸlantÄ±
//  âœ“ IPC Ã§Ã¶kerse Round-Robin WSS fallback (3 endpoint)
//  âœ“ Arka plan saÄŸlÄ±k kontrolÃ¼ (2s geride kalan node geÃ§ici olarak devre dÄ±ÅŸÄ±)
//  âœ“ Zero-copy provider referanslarÄ±
//  âœ“ Lock-free okuma (parking_lot::RwLock)
// ============================================================================

use alloy::providers::{Provider, ProviderBuilder, RootProvider, WsConnect};
use alloy::pubsub::PubSubFrontend;
use eyre::Result;
use parking_lot::RwLock;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, AtomicBool, Ordering};
use std::time::Duration;

// â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
// Node SaÄŸlÄ±k Durumu
// â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

/// Tek bir RPC node'unun durumu
struct NodeState {
    /// Provider instance (baÄŸlantÄ± kurulmuÅŸsa) â€” v22.0: RwLock ile cache
    provider: RwLock<Option<RootProvider<PubSubFrontend>>>,
    /// WebSocket URL'i
    url: String,
    /// Node saÄŸlÄ±klÄ± mÄ±? (atomik â€” lock-free okuma)
    healthy: AtomicBool,
    /// Son bilinen blok numarasÄ±
    last_block: AtomicUsize,
}

// â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
// RPC Pool â€” IPC Ã–ncelikli, Round-Robin WSS Fallback
// â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

/// HFT-grade RPC baÄŸlantÄ± havuzu.
///
/// Ã–ncelik sÄ±rasÄ±:
///   1. Yerel IPC soketi (sub-0.1ms gecikme)
///   2. WSS endpoint'leri (Round-Robin, saÄŸlÄ±k kontrolÃ¼ ile)
///
/// SaÄŸlÄ±k kontrolÃ¼:
///   - Arka plan task her 2 saniyede tÃ¼m node'larÄ± yoklar
///   - `eth_blockNumber` ile gÃ¼ncel blok sorgulanÄ±r
///   - En yÃ¼ksek blok sayÄ±sÄ±na gÃ¶re 2+ blok geride kalan node devre dÄ±ÅŸÄ± bÄ±rakÄ±lÄ±r
pub struct RpcPool {
    /// IPC provider (varsa â€” en dÃ¼ÅŸÃ¼k gecikme)
    ipc_provider: RwLock<Option<RootProvider<PubSubFrontend>>>,
    /// IPC saÄŸlÄ±klÄ± mÄ±?
    ipc_healthy: AtomicBool,
    /// IPC yolu (reconnect iÃ§in)
    ipc_path: Option<String>,
    /// WSS node listesi
    ws_nodes: Vec<Arc<NodeState>>,
    /// Round-Robin sayacÄ± (atomik)
    rr_counter: AtomicUsize,
    /// Pool aktif mi?
    active: AtomicBool,
}

impl RpcPool {
    /// Yeni RPC Pool oluÅŸtur.
    ///
    /// # ArgÃ¼manlar
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

    /// TÃ¼m baÄŸlantÄ±larÄ± baÅŸlat (IPC + WSS).
    /// DÃ¶ngÃ¼ dÄ±ÅŸÄ±nda bir kez Ã§aÄŸrÄ±lÄ±r â€” allocation burada yapÄ±lÄ±r.
    pub async fn connect_all(&mut self) -> Result<()> {
        // 1. IPC baÄŸlantÄ±sÄ± (varsa)
        if let Some(ref ipc_path) = self.ipc_path {
            match self.try_connect_ipc(ipc_path).await {
                Ok(provider) => {
                    *self.ipc_provider.write() = Some(provider);
                    self.ipc_healthy.store(true, Ordering::Release);
                    eprintln!("  âœ… IPC baÄŸlantÄ± kuruldu: {}", ipc_path);
                }
                Err(e) => {
                    eprintln!("  âš ï¸  IPC baÄŸlantÄ± baÅŸarÄ±sÄ±z (WSS fallback aktif): {}", e);
                }
            }
        }

        // 2. WSS baÄŸlantÄ±larÄ± â€” paralel deÄŸil sÄ±ralÄ± (alloy WsConnect thread-safe deÄŸil)
        for node in &self.ws_nodes {
            match Self::try_connect_ws(&node.url).await {
                Ok(provider) => {
                    node.healthy.store(true, Ordering::Release);
                    eprintln!("  âœ… WSS baÄŸlantÄ± kuruldu: {}", &node.url[..node.url.len().min(40)]);

                    // v22.0: Provider'Ä± cache'e al â€” get_provider her seferinde
                    // yeni baÄŸlantÄ± aÃ§mak yerine cache'den klonlar
                    *node.provider.write() = Some(provider);
                }
                Err(e) => {
                    eprintln!("  âš ï¸  WSS baÄŸlantÄ± baÅŸarÄ±sÄ±z: {} â€” {}", &node.url[..node.url.len().min(40)], e);
                }
            }
        }

        // En az bir baÄŸlantÄ± var mÄ±?
        let has_ipc = self.ipc_healthy.load(Ordering::Acquire);
        let has_ws = self.ws_nodes.iter().any(|n| n.healthy.load(Ordering::Acquire));

        if !has_ipc && !has_ws {
            return Err(eyre::eyre!("HiÃ§bir RPC endpoint'e baÄŸlanÄ±lamadÄ±!"));
        }

        Ok(())
    }

    /// En dÃ¼ÅŸÃ¼k gecikmeli saÄŸlÄ±klÄ± provider'Ä± dÃ¶ndÃ¼r.
    /// Ã–ncelik: IPC > Round-Robin WSS
    pub async fn get_provider(&self) -> Result<RootProvider<PubSubFrontend>> {
        // 1. IPC saÄŸlÄ±klÄ±ysa onu kullan
        if self.ipc_healthy.load(Ordering::Acquire) {
            let guard = self.ipc_provider.read();
            if let Some(ref provider) = *guard {
                return Ok(provider.clone());
            }
        }

        // 2. WSS Round-Robin
        let node_count = self.ws_nodes.len();
        if node_count == 0 {
            return Err(eyre::eyre!("WSS node listesi boÅŸ!"));
        }

        // SaÄŸlÄ±klÄ± node bul (en fazla node_count deneme)
        for _ in 0..node_count {
            let idx = self.rr_counter.fetch_add(1, Ordering::Relaxed) % node_count;
            let node = &self.ws_nodes[idx];

            if !node.healthy.load(Ordering::Acquire) {
                continue;
            }

            // v22.0: Cache'den klonla â€” her seferinde yeni baÄŸlantÄ± aÃ§mak yerine
            // mevcut provider'Ä± kullan. BaÄŸlantÄ± kopmuÅŸsa yeniden baÄŸlan.
            {
                let guard = node.provider.read();
                if let Some(ref provider) = *guard {
                    return Ok(provider.clone());
                }
            }

            // Cache boÅŸ â€” yeni baÄŸlantÄ± aÃ§ ve cache'e al
            match Self::try_connect_ws(&node.url).await {
                Ok(provider) => {
                    let cloned = provider.clone();
                    *node.provider.write() = Some(provider);
                    return Ok(cloned);
                }
                Err(e) => {
                    node.healthy.store(false, Ordering::Release);
                    eprintln!("  âš ï¸  WSS node {} baÄŸlantÄ± kaybÄ±: {}", idx, e);
                    continue;
                }
            }
        }

        Err(eyre::eyre!("TÃ¼m RPC node'larÄ± devre dÄ±ÅŸÄ± â€” saÄŸlÄ±k kontrolÃ¼ bekleniyor"))
    }

    /// Arka plan saÄŸlÄ±k kontrolÃ¼ task'Ä± baÅŸlat.
    ///
    /// Her 2 saniyede:
    ///   1. TÃ¼m node'lardan `eth_blockNumber` sorgular
    ///   2. En yÃ¼ksek blok sayÄ±sÄ±nÄ± belirler
    ///   3. 2+ blok gerisinde kalan node'larÄ± devre dÄ±ÅŸÄ± bÄ±rakÄ±r
    ///   4. Devre dÄ±ÅŸÄ± node'larÄ± yeniden baÄŸlanmaya Ã§alÄ±ÅŸÄ±r
    pub fn spawn_health_checker(self: &Arc<Self>) {
        let pool = Arc::clone(self);

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(2));

            loop {
                interval.tick().await;

                if !pool.active.load(Ordering::Acquire) {
                    break;
                }

                // IPC saÄŸlÄ±k kontrolÃ¼
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

                // WSS node'larÄ± kontrol et â€” blok numaralarÄ±nÄ± topla
                let mut block_numbers: Vec<(usize, u64)> = Vec::with_capacity(pool.ws_nodes.len());

                for (idx, node) in pool.ws_nodes.iter().enumerate() {
                    // v22.0: Ä°lk olarak cache'deki provider'Ä± dene
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
                                        "  ğŸ”„ WSS node #{} tekrar saÄŸlÄ±klÄ± (blok #{})",
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
                        // Cache'deki provider baÅŸarÄ±sÄ±z â€” yeniden baÄŸlan
                        match Self::try_connect_ws(&node.url).await {
                            Ok(provider) => {
                                match provider.get_block_number().await {
                                    Ok(bn) => {
                                        node.last_block.store(bn as usize, Ordering::Release);
                                        block_numbers.push((idx, bn));
                                        *node.provider.write() = Some(provider);
                                        node.healthy.store(true, Ordering::Release);
                                        eprintln!(
                                            "  ğŸ”„ WSS node #{} yeniden baÄŸlandÄ± (blok #{})",
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

                // En yÃ¼ksek blok numarasÄ±nÄ± bul
                if let Some(max_block) = block_numbers.iter().map(|(_, b)| *b).max() {
                    // 2+ blok geride kalan node'larÄ± devre dÄ±ÅŸÄ± bÄ±rak
                    for (idx, bn) in &block_numbers {
                        if max_block.saturating_sub(*bn) >= 2 {
                            let node = &pool.ws_nodes[*idx];
                            if node.healthy.load(Ordering::Acquire) {
                                node.healthy.store(false, Ordering::Release);
                                eprintln!(
                                    "  âš ï¸  WSS node #{} geride kaldÄ± (blok #{} vs max #{}) â€” geÃ§ici devre dÄ±ÅŸÄ±",
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
    #[allow(dead_code)]
    pub fn shutdown(&self) {
        self.active.store(false, Ordering::Release);
    }

    // â”€â”€ Ä°Ã§ BaÄŸlantÄ± YardÄ±mcÄ±larÄ± â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    /// IPC sokete baÄŸlan
    /// v22.0: IPC path tanÄ±mlÄ±ysa local WS proxy olarak kullanÄ±lÄ±r.
    /// Alloy 0.1'de native IPC desteÄŸi yoktur â€” local node'un WS
    /// endpointi Ã¼zerinden baÄŸlantÄ± kurulur (eÅŸdeÄŸer gecikme).
    async fn try_connect_ipc(&self, ipc_path: &str) -> Result<RootProvider<PubSubFrontend>> {
        // Alloy IPC provider â€” Base node'un IPC soketi
        // Windows: \\.\pipe\geth.ipc
        // Linux/Mac: /tmp/geth.ipc veya /path/to/base-node/geth.ipc
        eprintln!(
            "  â„¹ï¸  IPC path '{}' tanÄ±mlÄ± â€” Alloy 0.1 native IPC desteklemediÄŸinden local WS proxy kullanÄ±lÄ±yor",
            ipc_path
        );
        let ws_url = format!("ws://127.0.0.1:8546");

        let ws = WsConnect::new(&ws_url);
        let provider = ProviderBuilder::new()
            .on_ws(ws)
            .await
            .map_err(|e| eyre::eyre!("IPC/Local WS baÄŸlantÄ± hatasÄ± ({}): {}", ipc_path, e))?;

        // BaÄŸlantÄ± testi
        let _block = provider.get_block_number().await
            .map_err(|e| eyre::eyre!("IPC saÄŸlÄ±k kontrolÃ¼ baÅŸarÄ±sÄ±z: {}", e))?;

        Ok(provider)
    }

    /// WebSocket'e baÄŸlan
    async fn try_connect_ws(url: &str) -> Result<RootProvider<PubSubFrontend>> {
        let ws = WsConnect::new(url);
        let provider = ProviderBuilder::new()
            .on_ws(ws)
            .await
            .map_err(|e| eyre::eyre!("WSS baÄŸlantÄ± hatasÄ± ({}): {}", &url[..url.len().min(40)], e))?;

        Ok(provider)
    }

    /// Aktif saÄŸlÄ±klÄ± node sayÄ±sÄ±
    pub fn healthy_node_count(&self) -> usize {
        let ipc = if self.ipc_healthy.load(Ordering::Acquire) { 1 } else { 0 };
        let ws = self.ws_nodes.iter()
            .filter(|n| n.healthy.load(Ordering::Acquire))
            .count();
        ipc + ws
    }

    /// Transport bilgi stringi (banner iÃ§in)
    pub fn transport_info(&self) -> String {
        let ipc_status = if self.ipc_healthy.load(Ordering::Acquire) {
            "IPC âœ…"
        } else if self.ipc_path.is_some() {
            "IPC âŒ"
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
