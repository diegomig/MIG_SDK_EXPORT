// Quick test to verify Redis and PostgreSQL connections
use std::env;
use sqlx::Row;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv::dotenv().ok();
    
    println!("🔍 Testing database and Redis connections...\n");
    
    // Test PostgreSQL connection
    let database_url = env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://mig_topology_user:mig_topology_pass@localhost:5432/mig_topology".to_string());
    
    println!("📊 Testing PostgreSQL connection...");
    println!("   URL: {}", database_url.replace(&env::var("POSTGRES_PASSWORD").unwrap_or_default(), "***"));
    
    match sqlx::PgPool::connect(&database_url).await {
        Ok(pool) => {
            match sqlx::query("SELECT version()").fetch_one(&pool).await {
                Ok(row) => {
                    let version: String = row.get(0);
                    println!("   ✅ PostgreSQL connected successfully!");
                    println!("   Version: {}", version.split('\n').next().unwrap_or("Unknown"));
                    
                    // Test pool count
                    match sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM mig_topology.pools WHERE is_valid = true")
                        .fetch_one(&pool).await {
                        Ok(count) => println!("   ✅ Valid pools in DB: {}", count),
                        Err(e) => println!("   ⚠️  Could not query pools: {}", e),
                    }
                }
                Err(e) => {
                    println!("   ❌ PostgreSQL query failed: {}", e);
                    return Err(e.into());
                }
            }
        }
        Err(e) => {
            println!("   ❌ PostgreSQL connection failed: {}", e);
            return Err(e.into());
        }
    }
    
    println!();
    
    // Test Redis connection
    #[cfg(feature = "redis")]
    {
        use mig_topology_sdk::redis_manager::{RedisManager, RedisConfig};
        
        let redis_url = env::var("REDIS_URL")
            .unwrap_or_else(|_| "redis://localhost:6379".to_string());
        
        println!("🔴 Testing Redis connection...");
        println!("   URL: {}", redis_url);
        
        match RedisManager::new(RedisConfig {
            url: redis_url.clone(),
            pool_state_ttl: 60,
            route_cache_ttl: 60,
        }).await {
            Ok(mut redis_mgr) => {
                println!("   ✅ Redis Manager initialized successfully!");
                
                // Test health check (PING)
                match redis_mgr.health_check().await {
                    Ok(_) => {
                        println!("   ✅ Redis health check (PING) successful");
                        
                        // Get Redis info
                        match redis_mgr.get_info().await {
                            Ok(info) => {
                                // Extract some useful info
                                if let Some(keyspace_line) = info.lines().find(|l| l.starts_with("keyspace")) {
                                    println!("   ℹ️  {}", keyspace_line);
                                }
                            }
                            Err(e) => println!("   ⚠️  Could not get Redis info: {}", e),
                        }
                    }
                    Err(e) => {
                        println!("   ❌ Redis health check failed: {}", e);
                        return Err(e.into());
                    }
                }
            }
            Err(e) => {
                println!("   ❌ Redis connection failed: {}", e);
                return Err(e.into());
            }
        }
    }
    
    #[cfg(not(feature = "redis"))]
    {
        println!("🔴 Redis feature not enabled, skipping Redis test");
    }
    
    println!("\n✅ All connection tests completed!");
    Ok(())
}
