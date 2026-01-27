// examples/test_concurrent_rpc.rs
//
// Test para reproducir el problema de "EOF while parsing" en ambiente concurrente
// Este test simula las llamadas concurrentes que hace el orchestrator

use ethers::providers::{Http, Middleware, Provider};
use std::sync::Arc;
use std::time::Instant;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🧪 Test de Llamadas Concurrentes al RPC");
    println!("========================================\n");

    // RPC URLs (mismo que en el SDK)
    let rpc_urls = vec![
        "https://arb1.arbitrum.io/rpc",
        "https://arbitrum.llamarpc.com",
    ];

    for (idx, url) in rpc_urls.iter().enumerate() {
        println!("📡 Probando RPC #{}: {}", idx + 1, url);

        // Test 1: Llamada individual (baseline)
        println!("\n  Test 1: Llamada individual...");
        match test_single_call(url).await {
            Ok(block) => println!("    ✅ OK: block {}", block),
            Err(e) => println!("    ❌ ERROR: {}", e),
        }

        // Test 2: 5 llamadas secuenciales
        println!("\n  Test 2: 5 llamadas secuenciales...");
        match test_sequential_calls(url, 5).await {
            Ok(count) => println!("    ✅ OK: {} llamadas exitosas", count),
            Err(e) => println!("    ❌ ERROR: {}", e),
        }

        // Test 3: 10 llamadas concurrentes (simula orchestrator)
        println!("\n  Test 3: 10 llamadas CONCURRENTES...");
        match test_concurrent_calls(url, 10).await {
            Ok((success, failed)) => {
                if failed == 0 {
                    println!("    ✅ OK: {} llamadas exitosas", success);
                } else {
                    println!("    ⚠️  {} exitosas, {} fallidas", success, failed);
                }
            }
            Err(e) => println!("    ❌ ERROR: {}", e),
        }

        // Test 4: 20 llamadas muy concurrentes (stress test)
        println!("\n  Test 4: 20 llamadas MUY concurrentes (stress)...");
        match test_concurrent_calls(url, 20).await {
            Ok((success, failed)) => {
                if failed == 0 {
                    println!("    ✅ OK: {} llamadas exitosas", success);
                } else {
                    println!("    ⚠️  {} exitosas, {} fallidas", success, failed);
                }
            }
            Err(e) => println!("    ❌ ERROR: {}", e),
        }

        println!("\n  ----------------------------------------\n");
    }

    println!("\n✅ Tests completados");
    println!("\n💡 Interpretación de resultados:");
    println!("  - Si Test 1-2 OK pero Test 3-4 fallan → Problema de concurrencia");
    println!("  - Si Test 3 OK pero Test 4 falla → Rate limiting del RPC");
    println!("  - Si todos fallan → Problema de conectividad/RPC");
    println!("  - Si todos OK → Problema está en otra parte del código (no en RPC básico)");

    Ok(())
}

// Test 1: Una sola llamada
async fn test_single_call(url: &str) -> Result<u64, Box<dyn std::error::Error>> {
    let provider = Provider::<Http>::try_from(url)?;
    let block = provider.get_block_number().await?;
    Ok(block.as_u64())
}

// Test 2: Llamadas secuenciales
async fn test_sequential_calls(
    url: &str,
    count: usize,
) -> Result<usize, Box<dyn std::error::Error>> {
    let provider = Provider::<Http>::try_from(url)?;
    let mut success = 0;

    for i in 0..count {
        match provider.get_block_number().await {
            Ok(_) => {
                success += 1;
                print!(".");
            }
            Err(e) => {
                eprintln!("\n    ❌ Llamada {} falló: {}", i + 1, e);
            }
        }
    }
    println!();

    Ok(success)
}

// Test 3-4: Llamadas concurrentes (simula orchestrator)
async fn test_concurrent_calls(
    url: &str,
    count: usize,
) -> Result<(usize, usize), Box<dyn std::error::Error>> {
    let provider = Arc::new(Provider::<Http>::try_from(url)?);
    let start = Instant::now();

    let mut tasks = vec![];
    for i in 0..count {
        let p = provider.clone();
        let task = tokio::spawn(async move {
            match p.get_block_number().await {
                Ok(block) => {
                    // println!("    ✅ Task {}: block {}", i, block.as_u64());
                    Ok(())
                }
                Err(e) => {
                    eprintln!("    ❌ Task {}: {}", i, e);
                    // Verificar si es "EOF while parsing"
                    if e.to_string().contains("EOF while parsing") {
                        eprintln!("    🔍 EOF ERROR DETECTADO en task {}", i);
                    }
                    Err(e)
                }
            }
        });
        tasks.push(task);
    }

    let mut success = 0;
    let mut failed = 0;

    for task in tasks {
        match task.await {
            Ok(Ok(())) => success += 1,
            _ => failed += 1,
        }
    }

    let elapsed = start.elapsed();
    println!(
        "    ⏱️  Completado en {:?} ({} exitosas, {} fallidas)",
        elapsed, success, failed
    );

    Ok((success, failed))
}
