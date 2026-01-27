//! # Flight Recorder Test
//!
//! Simple test to verify that the Flight Recorder captures and writes events correctly.
//!
//! ## Usage
//!
//! ```bash
//! cargo run --example test_flight_recorder
//! ```

use mig_topology_sdk::flight_recorder::{flight_recorder_writer, FlightEvent, FlightRecorder};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🧪 Testing Flight Recorder...");
    println!("═══════════════════════════════════════════════════════\n");

    // 1. Create Flight Recorder
    let (recorder, event_rx) = FlightRecorder::new();
    println!("✅ Flight Recorder created");

    // 2. ✅ CRITICAL: Spawn writer task FIRST (before enabling)
    let output_file = "test_flight_recorder.jsonl".to_string();
    let output_file_for_writer = output_file.clone();
    println!("🎬 Spawning Flight Recorder writer task: {}", output_file);
    let _writer_handle = tokio::spawn(async move {
        match flight_recorder_writer(event_rx, output_file_for_writer).await {
            Ok(_) => println!("✅ Writer task completed successfully"),
            Err(e) => eprintln!("❌ Writer task error: {}", e),
        }
    });

    // Small delay to ensure writer task is ready
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // 3. Enable recorder
    recorder.enable();
    let (enabled, count_before) = recorder.stats();
    println!(
        "✅ Flight Recorder enabled: enabled={}, events={}",
        enabled, count_before
    );

    if !enabled {
        eprintln!("❌ ERROR: Flight Recorder is NOT enabled!");
        return Err("Flight Recorder failed to enable".into());
    }

    // 4. Record test events
    println!("\n📝 Recording 10 test events...");
    for i in 0..10 {
        recorder.record(FlightEvent::Decision {
            ts: 0,
            component: "test".to_string(),
            action: format!("action_{}", i),
            reason: "testing".to_string(),
            context: serde_json::json!({
                "iteration": i,
                "test": true
            }),
            block: Some(12345 + i),
        });
    }

    // 5. Check stats
    let (enabled, count_after, _, dropped) = recorder.stats_detailed();
    println!("\n📊 Flight Recorder Stats:");
    println!("   Enabled: {}", enabled);
    println!("   Events recorded: {}", count_after);
    println!("   Events dropped: {}", dropped);

    if dropped > 0 {
        eprintln!("⚠️  WARNING: {} events were dropped!", dropped);
    }

    if count_after < 10 {
        eprintln!(
            "⚠️  WARNING: Only {} events recorded (expected 10)",
            count_after
        );
    }

    // 6. Wait for flush
    println!("\n⏳ Waiting for events to be flushed to file...");
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

    // 7. Verify file exists and has content
    println!("\n🔍 Verifying output file...");
    match tokio::fs::read_to_string(&output_file).await {
        Ok(content) => {
            let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
            println!("✅ File exists: {}", output_file);
            println!("   Lines in file: {}", lines.len());
            println!("   File size: {} bytes", content.len());

            if lines.len() >= 10 {
                println!(
                    "\n✅ TEST PASSED: All {} events written to file",
                    lines.len()
                );

                // Show first event as example
                if !lines.is_empty() {
                    if let Ok(event_json) = serde_json::from_str::<serde_json::Value>(lines[0]) {
                        println!("\n📄 First event (example):");
                        println!("   {}", serde_json::to_string_pretty(&event_json)?);
                    }
                }
            } else {
                eprintln!(
                    "\n❌ TEST FAILED: Only {} events written (expected 10)",
                    lines.len()
                );
                return Err(format!("Expected 10 events, got {}", lines.len()).into());
            }
        }
        Err(e) => {
            eprintln!("❌ TEST FAILED: Could not read file {}: {}", output_file, e);
            return Err(format!("File read error: {}", e).into());
        }
    }

    // 8. Cleanup: Wait a bit more, then check if writer task completed
    println!("\n⏳ Waiting for writer task to complete...");
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Note: Writer task won't complete until the channel closes (all senders dropped)
    // Since we're still holding the recorder, the channel is still open.
    // This is expected behavior.

    println!("\n═══════════════════════════════════════════════════════");
    println!("✅ Flight Recorder test completed successfully!");
    println!("📁 Output file: {}", output_file);
    println!("💡 You can inspect the file to verify events were written correctly");

    Ok(())
}
