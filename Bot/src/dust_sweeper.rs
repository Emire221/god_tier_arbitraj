// ============================================================================
//  DUST SWEEPER v1.0 — Auto-Sweep Altcoin Dust to WETH
//
//
//  Periodically scans wallet for small ERC-20 balances (dust) and swaps them
//  to WETH via Uniswap V3 SwapRouter on Base L2.
//
//  Features:
//  ✓ Scans all whitelisted tokens for non-zero balances
//  ✓ Identifies dust below configurable threshold
//  ✓ Swaps via Uniswap V3 exactInputSingle (0.3% fee tier default)
//  ✓ MEV-protected via Private RPC (same as main executor)
//  ✓ CLI invocation: cargo run -- --sweep-dust
//  ✓ Dry-run by default (--sweep-dust --execute to actually send TXs)
// ============================================================================

use alloy::primitives::{address, Address, U256, Bytes, Uint};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use alloy::network::EthereumWallet;
use alloy::sol;
use alloy::sol_types::SolCall;
use eyre::Result;
use colored::*;

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// WETH address on Base
const WETH: Address = address!("4200000000000000000000000000000000000006");

/// Uniswap V3 SwapRouter02 on Base
const SWAP_ROUTER: Address = address!("2626664c2603336E57B271c5C0b26F421741e481");

/// Minimum dust value in USD equivalent to bother sweeping
/// Below this, gas cost exceeds swap value
const MIN_DUST_VALUE_USD: f64 = 0.50;

// ─────────────────────────────────────────────────────────────────────────────
// ABI Definitions (Solidity → Rust via alloy::sol!)
// ─────────────────────────────────────────────────────────────────────────────

sol! {
    /// ERC-20 balanceOf
    function balanceOf(address account) external view returns (uint256);

    /// ERC-20 decimals
    function decimals() external view returns (uint8);

    /// ERC-20 symbol
    function symbol() external view returns (string);

    /// ERC-20 approve
    function approve(address spender, uint256 amount) external returns (bool);

    /// ERC-20 allowance
    function allowance(address owner, address spender) external view returns (uint256);

    /// Uniswap V3 SwapRouter02 exactInputSingle
    struct ExactInputSingleParams {
        address tokenIn;
        address tokenOut;
        uint24 fee;
        address recipient;
        uint256 amountIn;
        uint256 amountOutMinimum;
        uint160 sqrtPriceLimitX96;
    }

    function exactInputSingle(ExactInputSingleParams params) external payable returns (uint256 amountOut);
}

// ─────────────────────────────────────────────────────────────────────────────
// Token Metadata
// ─────────────────────────────────────────────────────────────────────────────

/// Known token metadata for dust valuation (avoids extra RPC calls)
struct TokenMeta {
    address: Address,
    symbol: &'static str,
    decimals: u8,
    /// Approximate USD price (for dust threshold calculation)
    /// Updated manually; only needs to be ballpark accurate
    approx_usd: f64,
    /// Preferred fee tier for WETH swap (basis points × 100)
    fee_tier: u32,
}

/// Hardcoded metadata for whitelisted tokens on Base
/// This avoids extra RPC calls for symbol/decimals which are static
fn known_tokens() -> Vec<TokenMeta> {
    vec![
        // USDC — Circle (bridged) — deepest liquidity at 0.05% tier
        TokenMeta {
            address: address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"),
            symbol: "USDC",
            decimals: 6,
            approx_usd: 1.0,
            fee_tier: 500, // 0.05%
        },
        // USDbC — USD Base Coin (bridged)
        TokenMeta {
            address: address!("d9aAEc86B65D86f6A7B5B1b0c42FFA531710b6CA"),
            symbol: "USDbC",
            decimals: 6,
            approx_usd: 1.0,
            fee_tier: 500,
        },
        // DAI — Dai Stablecoin
        TokenMeta {
            address: address!("50c5725949A6F0c72E6C4a641F24049A917DB0Cb"),
            symbol: "DAI",
            decimals: 18,
            approx_usd: 1.0,
            fee_tier: 500,
        },
        // cbETH — Coinbase Wrapped Staked ETH
        TokenMeta {
            address: address!("2Ae3F1Ec7F1F5012CFEab0185bfc7aa3cf0DEc22"),
            symbol: "cbETH",
            decimals: 18,
            approx_usd: 2500.0, // ~ETH price
            fee_tier: 500,
        },
        // cbBTC — Coinbase Wrapped BTC
        TokenMeta {
            address: address!("cbB7C0000aB88B473b1f5aFd9ef808440eed33Bf"),
            symbol: "cbBTC",
            decimals: 8,
            approx_usd: 60000.0,
            fee_tier: 3000,
        },
        // AERO — Aerodrome Finance
        TokenMeta {
            address: address!("940181a94A35A4569E4529A3CDfB74e38FD98631"),
            symbol: "AERO",
            decimals: 18,
            approx_usd: 1.5,
            fee_tier: 3000,
        },
        // DEGEN — Degen token
        TokenMeta {
            address: address!("4ed4E862860beD51a9570b96d89aF5E1B0Efefed"),
            symbol: "DEGEN",
            decimals: 18,
            approx_usd: 0.01,
            fee_tier: 3000,
        },
    ]
}

// ─────────────────────────────────────────────────────────────────────────────
// Dust Info
// ─────────────────────────────────────────────────────────────────────────────

/// Detected dust token with balance info
struct DustEntry {
    meta: TokenMeta,
    balance_raw: U256,
    balance_human: f64,
    value_usd: f64,
}

// ─────────────────────────────────────────────────────────────────────────────
// Core Sweeper Logic
// ─────────────────────────────────────────────────────────────────────────────

/// Scan wallet and sweep dust tokens to WETH.
///
/// # Arguments
/// - `execute`: If true, actually send swap TXs. If false, dry-run (report only).
pub async fn run_sweep(execute: bool) -> Result<()> {
    println!();
    println!(
        "{}",
        "╔══════════════════════════════════════════════════════════════════╗"
            .cyan().bold()
    );
    println!(
        "{}",
        "║           DUST SWEEPER v1.0 — Auto-Sweep to WETH              ║"
            .cyan().bold()
    );
    println!(
        "{}",
        "╚══════════════════════════════════════════════════════════════════╝"
            .cyan().bold()
    );
    println!();

    if !execute {
        println!(
            "  {} DRY-RUN MODE — no transactions will be sent.",
            "👻".yellow()
        );
        println!(
            "  {} Add --execute flag to actually sweep: cargo run -- --sweep-dust --execute",
            "ℹ️".blue()
        );
        println!();
    }

    // ── 1. Load config ──
    let rpc_http_url = std::env::var("RPC_HTTP_URL")
        .map_err(|_| eyre::eyre!("RPC_HTTP_URL must be defined in .env!"))?;
    let private_rpc_url = std::env::var("PRIVATE_RPC_URL").ok().filter(|u| !u.is_empty());

    let private_key_str = get_private_key()?;
    let signer: PrivateKeySigner = private_key_str
        .parse()
        .map_err(|_| eyre::eyre!("Invalid private key format"))?;
    let wallet_address = signer.address();

    let addr_str = format!("{:?}", wallet_address);
    println!(
        "  {} Wallet: {}...{}",
        "🔑".green(),
        &addr_str[..8],
        &addr_str[38..]
    );

    // ── 2. Connect to RPC ──
    let rpc_url: reqwest::Url = rpc_http_url.parse()
        .map_err(|e| eyre::eyre!("RPC_HTTP_URL parse error: {}", e))?;
    let provider = ProviderBuilder::new().connect_http(rpc_url);

    // ── 3. Scan token balances ──
    let tokens = known_tokens();
    let mut dust_entries: Vec<DustEntry> = Vec::new();

    println!();
    println!("  {} Scanning {} whitelisted tokens...", "🔍".cyan(), tokens.len());
    println!("  {}", "─".repeat(60).dimmed());

    for meta in tokens {
        // Skip WETH — that's the target, not dust
        if meta.address == WETH {
            continue;
        }

        // Call balanceOf
        let calldata = balanceOfCall { account: wallet_address }.abi_encode();
        let tx = TransactionRequest::default()
            .to(meta.address)
            .input(Bytes::copy_from_slice(&calldata).into());

        let result = match provider.call(tx).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!(
                    "  {} {} balanceOf failed: {}",
                    "⚠️".yellow(), meta.symbol, e
                );
                continue;
            }
        };

        let balance = U256::from_be_slice(&result[result.len().saturating_sub(32)..]);
        if balance.is_zero() {
            continue;
        }

        let scale = 10f64.powi(meta.decimals as i32);
        let balance_human = balance.to::<u128>() as f64 / scale;
        let value_usd = balance_human * meta.approx_usd;

        let status = if value_usd >= MIN_DUST_VALUE_USD {
            "SWEEP".green().bold()
        } else {
            "SKIP (too small)".dimmed()
        };

        println!(
            "  {} {:>8} {:>16.6} (${:.2}) — {}",
            "•".cyan(), meta.symbol, balance_human, value_usd, status
        );

        if value_usd >= MIN_DUST_VALUE_USD {
            dust_entries.push(DustEntry {
                meta,
                balance_raw: balance,
                balance_human,
                value_usd,
            });
        }
    }

    println!("  {}", "─".repeat(60).dimmed());

    if dust_entries.is_empty() {
        println!(
            "\n  {} No sweepable dust found (all balances below ${:.2} threshold).",
            "✅".green(), MIN_DUST_VALUE_USD
        );
        return Ok(());
    }

    let total_usd: f64 = dust_entries.iter().map(|d| d.value_usd).sum();
    println!(
        "\n  {} Found {} tokens to sweep (total ~${:.2})",
        "💰".green(),
        dust_entries.len(),
        total_usd
    );

    if !execute {
        println!(
            "\n  {} Dry-run complete. Use --sweep-dust --execute to send transactions.",
            "👻".yellow()
        );
        return Ok(());
    }

    // ── 4. Execute swaps ──
    if execute && private_rpc_url.is_none() {
        return Err(eyre::eyre!(
            "PRIVATE_RPC_URL not defined! Sweeper requires MEV-protected TX sending. \
             Add PRIVATE_RPC_URL=https://... to .env."
        ));
    }

    let wallet = EthereumWallet::from(signer.clone());
    let send_url: reqwest::Url = private_rpc_url.as_ref().unwrap().parse()
        .map_err(|e| eyre::eyre!("PRIVATE_RPC_URL parse error: {}", e))?;
    let send_provider = ProviderBuilder::new()
        .wallet(wallet.clone())
        .connect_http(send_url);

    // Get current nonce
    let mut nonce = provider.get_transaction_count(wallet_address).await
        .map_err(|e| eyre::eyre!("Failed to get nonce: {}", e))?;

    // Get current block timestamp for deadline
    let block = provider.get_block_number().await
        .map_err(|e| eyre::eyre!("Failed to get block number: {}", e))?;

    let base_fee = {
        let latest = provider.get_block_by_number(alloy::eips::BlockNumberOrTag::Latest).await
            .map_err(|e| eyre::eyre!("Failed to get latest block: {}", e))?;
        match latest {
            Some(b) => b.header.base_fee_per_gas.unwrap_or(1_000_000_000),
            None => 1_000_000_000u64,
        }
    };

    println!();
    println!("  {} Executing {} swaps (block #{}, nonce={})...", "🚀".green(), dust_entries.len(), block, nonce);
    println!();

    let mut success_count = 0u32;
    let mut fail_count = 0u32;

    for entry in &dust_entries {
        println!(
            "  {} Sweeping {} {} (~${:.2}) → WETH...",
            "→".cyan(), entry.balance_human, entry.meta.symbol, entry.value_usd
        );

        // Step 1: Approve SwapRouter to spend tokens (if needed)
        let approval_params = ApprovalParams {
            token: entry.meta.address,
            owner: wallet_address,
            spender: SWAP_ROUTER,
            amount: entry.balance_raw,
            nonce,
            base_fee,
        };
        match ensure_approval(&provider, &send_provider, &approval_params).await {
            Ok(approval_used_nonce) => {
                if approval_used_nonce {
                    nonce += 1;
                }
            }
            Err(e) => {
                eprintln!("  {} Approval failed for {}: {}", "❌".red(), entry.meta.symbol, e);
                fail_count += 1;
                continue;
            }
        }

        // Step 2: Execute swap
        // 5% slippage tolerance for dust (small amounts, low liquidity pools)
        let min_out = entry.balance_raw / U256::from(20); // 5% of input as minimum output
        let swap_params = ExactInputSingleParams {
            tokenIn: entry.meta.address,
            tokenOut: WETH,
            fee: Uint::<24, 1>::from(entry.meta.fee_tier),
            recipient: wallet_address,
            amountIn: entry.balance_raw,
            amountOutMinimum: min_out,
            sqrtPriceLimitX96: Uint::<160, 3>::ZERO,
        };

        let swap_calldata = exactInputSingleCall { params: swap_params }.abi_encode();

        let gas_limit = 200_000u64; // Generous limit for single-hop swap
        let max_fee = (base_fee as u128).saturating_mul(2).max(1_000_000_000);

        let swap_tx = TransactionRequest::default()
            .to(SWAP_ROUTER)
            .input(Bytes::copy_from_slice(&swap_calldata).into())
            .nonce(nonce)
            .gas_limit(gas_limit)
            .max_fee_per_gas(max_fee)
            .max_priority_fee_per_gas(1_000_000u128); // Minimal priority (not competing)

        match send_provider.send_transaction(swap_tx).await {
            Ok(pending) => {
                let tx_hash = format!("{:?}", pending.tx_hash());
                println!(
                    "  {} {} → WETH swap sent: {}",
                    "✅".green(), entry.meta.symbol, &tx_hash[..12]
                );
                nonce += 1;
                success_count += 1;

                // Log to JSON
                crate::json_logger::log_json("trade", "dust_sweep", serde_json::json!({
                    "token": entry.meta.symbol,
                    "amount": entry.balance_human,
                    "value_usd": entry.value_usd,
                    "tx_hash": tx_hash,
                }));
            }
            Err(e) => {
                eprintln!(
                    "  {} {} swap failed: {}",
                    "❌".red(), entry.meta.symbol, e
                );
                fail_count += 1;
            }
        }
    }

    // ── 5. Summary ──
    println!();
    println!("  {}", "─".repeat(60).dimmed());
    println!(
        "  {} Sweep complete: {} success, {} failed",
        if fail_count == 0 { "✅".green() } else { "⚠️".yellow() },
        success_count,
        fail_count
    );

    crate::json_logger::log_json("info", "dust_sweep_summary", serde_json::json!({
        "tokens_swept": success_count,
        "tokens_failed": fail_count,
        "total_value_usd": total_usd,
    }));

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Get private key from keystore or env var
fn get_private_key() -> Result<String> {
    // Try keystore first
    let km = crate::key_manager::KeyManager::auto_load()?;
    if let Some(key) = km.private_key() {
        return Ok(key.to_string());
    }

    // Fallback to env var
    std::env::var("PRIVATE_KEY")
        .ok()
        .filter(|pk| !pk.is_empty())
        .ok_or_else(|| eyre::eyre!(
            "No private key available. Use --encrypt-key to create a keystore \
             or set PRIVATE_KEY in .env."
        ))
}

/// Parameters for token approval check/send
struct ApprovalParams {
    token: Address,
    owner: Address,
    spender: Address,
    amount: U256,
    nonce: u64,
    base_fee: u64,
}

/// Ensure the SwapRouter has sufficient allowance, approve if needed.
/// Returns Ok(true) if an approval TX was sent (nonce consumed), Ok(false) if already approved.
async fn ensure_approval<P1: Provider, P2: Provider>(
    read_provider: &P1,
    send_provider: &P2,
    params: &ApprovalParams,
) -> Result<bool> {
    // Check current allowance
    let allowance_calldata = allowanceCall { owner: params.owner, spender: params.spender }.abi_encode();
    let allowance_tx = TransactionRequest::default()
        .to(params.token)
        .input(Bytes::copy_from_slice(&allowance_calldata).into());

    let result = read_provider.call(allowance_tx).await
        .map_err(|e| eyre::eyre!("allowance check failed: {}", e))?;

    let current_allowance = U256::from_be_slice(&result[result.len().saturating_sub(32)..]);

    if current_allowance >= params.amount {
        return Ok(false); // Already approved
    }

    // Send approval TX (max uint256 to avoid repeated approvals)
    let approve_calldata = approveCall {
        spender: params.spender,
        amount: U256::MAX,
    }.abi_encode();

    let approve_tx = TransactionRequest::default()
        .to(params.token)
        .input(Bytes::copy_from_slice(&approve_calldata).into())
        .nonce(params.nonce)
        .gas_limit(60_000u64)
        .max_fee_per_gas((params.base_fee as u128).saturating_mul(2).max(1_000_000_000))
        .max_priority_fee_per_gas(1_000_000u128);

    let _pending = send_provider.send_transaction(approve_tx).await
        .map_err(|e| eyre::eyre!("approve TX failed: {}", e))?;

    // Brief wait for approval to land (Base L2 ~2s blocks)
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    Ok(true)
}
