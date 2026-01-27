//! Independent RPC connection test
//! Tests RPC endpoints directly to diagnose connection issues

use anyhow::Result;
use ethers::prelude::*;
use ethers::types::{BlockId, BlockNumber, Filter};
use mig_topology_sdk::contracts::{i_uniswap_v2_factory::PairCreatedFilter, IUniswapV2Factory};
use std::sync::Arc;
use std::time::Instant;

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    println!("🧪 RPC Connection Test - Independent Diagnostic\n");

    // Load RPC URLs from environment
    let http_urls = std::env::var("SDK_RPC_HTTP_URLS").unwrap_or_else(|_| "[]".to_string());

    println!("📋 Configuration:");
    println!("   SDK_RPC_HTTP_URLS: {}\n", http_urls);

    // Parse using same logic as Settings::parse_string_list
    let urls: Vec<String> = parse_string_list(&http_urls)
        .ok_or_else(|| anyhow::anyhow!("Failed to parse SDK_RPC_HTTP_URLS"))?;

    if urls.is_empty() {
        eprintln!("❌ No RPC URLs found in SDK_RPC_HTTP_URLS");
        eprintln!("   Expected format: [\"https://arb-mainnet.g.alchemy.com/v2/YOUR_KEY\", ...]");
        return Ok(());
    }

    println!("✅ Found {} RPC URL(s)\n", urls.len());

    // Test each URL
    for (idx, url) in urls.iter().enumerate() {
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("🔍 Testing RPC #{}: {}", idx + 1, mask_url(url));
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

        match test_rpc_url(url).await {
            Ok(_) => {
                println!("✅ RPC #{}: All tests passed!\n", idx + 1);
            }
            Err(e) => {
                eprintln!("❌ RPC #{}: Failed with error:\n   {}\n", idx + 1, e);
            }
        }
    }

    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("✅ Test complete!");
    Ok(())
}

async fn test_rpc_url(url: &str) -> Result<()> {
    // Create provider
    println!("1️⃣ Creating provider...");
    let provider = Provider::<Http>::try_from(url)?;
    let provider = Arc::new(provider);
    println!("   ✅ Provider created\n");

    // Test 1: get_block_number
    println!("2️⃣ Testing get_block_number()...");
    let start = Instant::now();
    match provider.get_block_number().await {
        Ok(block) => {
            let elapsed = start.elapsed();
            println!("   ✅ Block number: {}", block);
            println!("   ⏱️  Latency: {:?}\n", elapsed);
        }
        Err(e) => {
            return Err(anyhow::anyhow!("get_block_number failed: {}", e));
        }
    }

    // Test 2: get_block (Latest)
    println!("3️⃣ Testing get_block(BlockId::Number(BlockNumber::Latest))...");
    let start = Instant::now();
    match provider
        .get_block(BlockId::Number(BlockNumber::Latest))
        .await
    {
        Ok(Some(block)) => {
            let elapsed = start.elapsed();
            println!(
                "   ✅ Block retrieved: #{}",
                block.number.unwrap_or_default()
            );
            println!("   ⏱️  Latency: {:?}\n", elapsed);
        }
        Ok(None) => {
            return Err(anyhow::anyhow!("get_block returned None"));
        }
        Err(e) => {
            return Err(anyhow::anyhow!("get_block failed: {}", e));
        }
    }

    // Test 3: get_logs with a simple filter (last 100 blocks)
    println!("4️⃣ Testing get_logs() with simple filter...");
    let current_block = provider.get_block_number().await?.as_u64();
    let from_block = current_block.saturating_sub(100);

    println!("   Block range: {} to {}", from_block, current_block);

    let filter = Filter::new().from_block(from_block).to_block(current_block);

    let start = Instant::now();
    match provider.get_logs(&filter).await {
        Ok(logs) => {
            let elapsed = start.elapsed();
            println!("   ✅ get_logs successful: {} logs found", logs.len());
            println!("   ⏱️  Latency: {:?}\n", elapsed);
        }
        Err(e) => {
            let elapsed = start.elapsed();
            eprintln!("   ❌ get_logs failed after {:?}", elapsed);
            eprintln!("   Error: {}", e);
            eprintln!("   Error type: {:?}\n", e);

            // Try to get more details
            let error_str = format!("{}", e);
            if error_str.contains("EOF") || error_str.contains("empty") {
                eprintln!("   ⚠️  This looks like the same error we're seeing in the SDK!");
                eprintln!("   The RPC is returning an empty response.");
            }
            return Err(anyhow::anyhow!("get_logs failed: {}", e));
        }
    }

    // Test 4: Event query using ethers contract (like UniswapV2Adapter does)
    println!("5️⃣ Testing event query (PairCreated from UniswapV2 Factory)...");

    // UniswapV2 Factory address on Arbitrum
    let factory_address: Address = "0xf1D7CC64Fb4452F05c498126312eBE29f30Fbcf9"
        .parse()
        .unwrap();

    println!("   Factory address: {:?}", factory_address);
    println!("   Block range: {} to {}", from_block, current_block);

    // Create contract instance
    let factory = IUniswapV2Factory::new(factory_address, Arc::clone(&provider));

    // Create event filter
    let event_filter = factory
        .event::<PairCreatedFilter>()
        .from_block(from_block)
        .to_block(current_block);

    let start = Instant::now();
    match event_filter.query().await {
        Ok(logs) => {
            let elapsed = start.elapsed();
            println!(
                "   ✅ Event query successful: {} PairCreated events found",
                logs.len()
            );
            println!("   ⏱️  Latency: {:?}\n", elapsed);
        }
        Err(e) => {
            let elapsed = start.elapsed();
            eprintln!("   ❌ Event query failed after {:?}", elapsed);
            eprintln!("   Error: {}", e);
            eprintln!("   Error type: {:?}\n", e);

            // Check for specific error patterns
            let error_str = format!("{}", e);
            if error_str.contains("EOF")
                || error_str.contains("empty")
                || error_str.contains("EOF while parsing")
            {
                eprintln!("   ⚠️  ⚠️  ⚠️  THIS IS THE ERROR WE'RE INVESTIGATING! ⚠️  ⚠️  ⚠️");
                eprintln!("   The RPC is returning an empty response to event queries.");
                eprintln!("   This suggests:");
                eprintln!("     - Rate limiting (check your RPC plan)");
                eprintln!("     - Invalid API key or authentication");
                eprintln!("     - RPC endpoint issue");
                eprintln!("     - Block range too large");
            }
            return Err(anyhow::anyhow!("Event query failed: {}", e));
        }
    }

    Ok(())
}

fn parse_string_list(input: &str) -> Option<Vec<String>> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Some(vec![]);
    }

    // Si parece JSON (empieza con '['), intentar parsear como JSON
    if trimmed.starts_with('[') {
        match serde_json::from_str::<Vec<String>>(trimmed) {
            Ok(v) => return Some(v),
            Err(_) => {
                // Fallback: Remover corchetes y parsear manualmente
                let without_brackets = trimmed.trim_start_matches('[').trim_end_matches(']').trim();

                // Si después de quitar corchetes tiene comillas, parsear como JSON string
                if without_brackets.starts_with('"') || without_brackets.starts_with('\'') {
                    let parts: Vec<String> = without_brackets
                        .split(',')
                        .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    return Some(parts);
                }

                // Si no tiene comillas, es una URL directa o lista separada por comas
                if !without_brackets.is_empty() {
                    let parts: Vec<String> = without_brackets
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    return Some(parts);
                }
            }
        }
    }

    // Fallback final: separar por coma
    let parts: Vec<String> = trimmed
        .split(',')
        .map(|s| s.trim().trim_matches('"').to_string())
        .filter(|s| !s.is_empty())
        .collect();
    Some(parts)
}

fn mask_url(url: &str) -> String {
    // Mask API keys in URLs for logging
    if url.contains("alchemy.com") {
        if let Some(pos) = url.find("/v2/") {
            let prefix = &url[..pos + 4];
            let rest = &url[pos + 4..];
            if rest.len() > 8 {
                format!("{}...{}", prefix, &rest[rest.len() - 8..])
            } else {
                format!("{}...", prefix)
            }
        } else {
            url.to_string()
        }
    } else if url.contains("infura.io") {
        if let Some(pos) = url.find("/v3/") {
            let prefix = &url[..pos + 4];
            let rest = &url[pos + 4..];
            if rest.len() > 8 {
                format!("{}...{}", prefix, &rest[rest.len() - 8..])
            } else {
                format!("{}...", prefix)
            }
        } else {
            url.to_string()
        }
    } else {
        url.to_string()
    }
}
