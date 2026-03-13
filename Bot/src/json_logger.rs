// ============================================================================
//  JSON LOGGER v1.0 — Structured JSON Logging for Machine Analysis
//
//  Writes structured JSON log entries to bot_logs.jsonl alongside terminal
//  output. Each line is a self-contained JSON object with:
//    - timestamp (ISO 8601)
//    - level (info, warn, error, trade, opportunity, stats)
//    - event name
//    - structured data fields
//
//  File is append-only, auto-rotated at 50MB.
// ============================================================================

use chrono::Local;
use serde_json::json;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Mutex;

const LOG_FILE: &str = "bot_logs.jsonl";
const MAX_LOG_SIZE_BYTES: u64 = 50 * 1024 * 1024; // 50MB

static LOGGER: std::sync::LazyLock<Mutex<JsonLogger>> =
    std::sync::LazyLock::new(|| Mutex::new(JsonLogger::new()));

pub struct JsonLogger {
    path: String,
}

impl JsonLogger {
    fn new() -> Self {
        Self {
            path: LOG_FILE.to_string(),
        }
    }

    fn rotate_if_needed(&self) {
        if let Ok(meta) = std::fs::metadata(&self.path) {
            if meta.len() > MAX_LOG_SIZE_BYTES {
                let rotated = format!("{}.{}.bak", self.path,
                    Local::now().format("%Y%m%d_%H%M%S"));
                let _ = std::fs::rename(&self.path, &rotated);
            }
        }
    }

    fn write_entry(&self, entry: &serde_json::Value) {
        self.rotate_if_needed();
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path);
        if let Ok(mut f) = file {
            if let Ok(line) = serde_json::to_string(entry) {
                let _ = writeln!(f, "{}", line);
            }
        }
    }
}

/// Log a structured JSON entry to bot_logs.jsonl
pub fn log_json(level: &str, event: &str, data: serde_json::Value) {
    let entry = json!({
        "ts": Local::now().format("%Y-%m-%dT%H:%M:%S%.3f").to_string(),
        "level": level,
        "event": event,
        "data": data,
    });
    if let Ok(logger) = LOGGER.lock() {
        logger.write_entry(&entry);
    }
}

/// Log block processing event
pub fn log_block(block_number: u64, sync_ms: u128, pool_count: usize) {
    log_json("info", "block_processed", json!({
        "block": block_number,
        "sync_ms": sync_ms,
        "pool_count": pool_count,
    }));
}

/// Log arbitrage opportunity (profitable or not)
pub fn log_opportunity(
    pair_name: &str,
    spread_pct: f64,
    profit_weth: f64,
    optimal_amount: f64,
    is_profitable: bool,
) {
    log_json(
        if is_profitable { "trade" } else { "info" },
        "opportunity",
        json!({
            "pair": pair_name,
            "spread_pct": spread_pct,
            "profit_weth": profit_weth,
            "optimal_amount_weth": optimal_amount,
            "profitable": is_profitable,
        }),
    );
}

/// Log trade execution result
#[allow(dead_code)]
pub fn log_trade(
    pair_name: &str,
    amount_weth: f64,
    profit_weth: f64,
    tx_hash: &str,
    success: bool,
    gas_used: u64,
) {
    log_json("trade", "execution", json!({
        "pair": pair_name,
        "amount_weth": amount_weth,
        "profit_weth": profit_weth,
        "tx_hash": tx_hash,
        "success": success,
        "gas_used": gas_used,
    }));
}

/// Log session statistics snapshot
pub fn log_stats(
    uptime: &str,
    blocks: u64,
    opportunities: u64,
    profitable: u64,
    executed: u64,
    total_profit: f64,
    avg_latency: f64,
) {
    log_json("stats", "session_snapshot", json!({
        "uptime": uptime,
        "blocks_processed": blocks,
        "opportunities_detected": opportunities,
        "profitable_opportunities": profitable,
        "executed_trades": executed,
        "total_potential_profit_weth": total_profit,
        "avg_latency_ms": avg_latency,
    }));
}

/// Log errors and warnings
#[allow(dead_code)]
pub fn log_error(event: &str, message: &str) {
    log_json("error", event, json!({
        "message": message,
    }));
}

/// Log pool discovery events
#[allow(dead_code)]
pub fn log_discovery(event: &str, pool_count: usize, source: &str) {
    log_json("info", event, json!({
        "pool_count": pool_count,
        "source": source,
    }));
}
