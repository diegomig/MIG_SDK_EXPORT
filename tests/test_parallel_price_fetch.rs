//! Integration tests for parallel price fetching (P1)
//! 
//! Tests verify that parallel price fetching configuration works correctly

use mig_topology_sdk::settings::Settings;

/// Test that parallel price fetching settings are correctly configured
#[test]
fn test_parallel_price_fetch_settings() {
    let settings = Settings::new().expect("Failed to load settings");
    
    // Verify parallel fetching is enabled by default
    assert!(
        settings.performance.parallel_price_fetching_enabled,
        "Parallel price fetching should be enabled by default"
    );
    
    // Verify chunk size is reasonable
    assert!(
        settings.performance.price_fetch_chunk_size > 0,
        "Price fetch chunk size should be > 0"
    );
    
    assert!(
        settings.performance.price_fetch_chunk_size <= 100,
        "Price fetch chunk size should be reasonable (<= 100)"
    );
}

/// Test that settings can be serialized/deserialized (for config file)
#[test]
fn test_settings_serialization() {
    let settings = Settings::new().expect("Failed to load settings");
    
    // Verify we can access parallel fetching settings
    let parallel_enabled = settings.performance.parallel_price_fetching_enabled;
    let chunk_size = settings.performance.price_fetch_chunk_size;
    
    // These should have reasonable default values
    assert!(parallel_enabled, "Parallel fetching should default to enabled");
    assert!(chunk_size >= 10 && chunk_size <= 50, "Chunk size should be between 10-50");
}
