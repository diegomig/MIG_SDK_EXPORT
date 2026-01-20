#!/usr/bin/env python3
import json
import sys
from collections import defaultdict
from pathlib import Path
import glob

# Find latest flight recorder file
benchmark_files = sorted(glob.glob("benchmarks/flight_recorder_*.jsonl"), reverse=True)
if not benchmark_files:
    print("❌ No flight recorder files found")
    sys.exit(1)

latest_file = benchmark_files[0]
print(f"📊 Analyzing: {latest_file}")
print("═══════════════════════════════════════════════════════════════════\n")

# Load events
events = []
with open(latest_file, 'r') as f:
    for line in f:
        try:
            events.append(json.loads(line.strip()))
        except:
            continue

print(f"📊 Total events: {len(events)}\n")

# Event types distribution
event_types = defaultdict(int)
for e in events:
    event_types[e.get('type', 'unknown')] += 1

print("📊 Event Types Distribution:")
for etype, count in sorted(event_types.items(), key=lambda x: -x[1]):
    print(f"  {count:5d} {etype}")
print()

# Weight refresh events
weight_refresh_events = [
    e for e in events 
    if e.get('phase') in ['weight_refresh_hot', 'weight_refresh_warm'] or
       (e.get('phase') == 'graph_updates' and 
        (e.get('metadata', {}).get('mode') == 'full' or 
         e.get('result', {}).get('mode') == 'full'))
]

if weight_refresh_events:
    print("📊 Weight Refresh Events:")
    phases = defaultdict(list)
    for e in weight_refresh_events:
        phase = e.get('phase', 'unknown')
        duration = e.get('duration_ms', 0)
        phases[phase].append(duration)
    
    for phase, durations in phases.items():
        avg_duration = sum(durations) / len(durations) if durations else 0
        print(f"  {phase}: {len(durations)} events, avg {avg_duration:.1f}ms")
        # Show sample result
        sample = next((e for e in weight_refresh_events if e.get('phase') == phase), None)
        if sample and 'result' in sample:
            result = sample['result']
            if isinstance(result, dict):
                pools = result.get('pools_updated', result.get('total_pools', 0))
                print(f"    Sample: {pools} pools updated")
    print()

# Graph updates
graph_updates = [e for e in events if e.get('phase') == 'graph_updates']
if graph_updates:
    print("📊 Graph Updates:")
    modes = defaultdict(list)
    for e in graph_updates:
        mode = (e.get('metadata', {}).get('mode') or 
                e.get('result', {}).get('mode') or 'unknown')
        modes[mode].append(e)
    
    for mode, mode_events in modes.items():
        durations = [e.get('duration_ms', 0) for e in mode_events]
        avg_duration = sum(durations) / len(durations) if durations else 0
        
        pools_list = []
        for e in mode_events:
            result = e.get('result', {})
            pools = (result.get('total_pools') or 
                    result.get('pools_updated') or 
                    result.get('total_updated') or 0)
            pools_list.append(pools)
        
        avg_pools = sum(pools_list) / len(pools_list) if pools_list else 0
        print(f"  {mode}: {len(mode_events)} events, avg {avg_duration:.1f}ms, avg {avg_pools:.0f} pools")
    print()

# Cache events
cache_events = [e for e in events if e.get('type') == 'CacheEvent']
if cache_events:
    print("📊 Cache Events:")
    cache_types = defaultdict(int)
    for e in cache_events:
        cache_types[e.get('event_type', 'unknown')] += 1
    
    total_cache = sum(cache_types.values())
    hits = cache_types.get('hit', 0)
    misses = cache_types.get('miss', 0)
    hit_rate = (hits / total_cache * 100) if total_cache > 0 else 0
    
    for ctype, count in sorted(cache_types.items(), key=lambda x: -x[1]):
        print(f"  {ctype}: {count}")
    print(f"  Cache Hit Rate: {hit_rate:.1f}%")
    print()

# RPC calls
rpc_calls = [e for e in events if e.get('type') == 'RpcCall']
if rpc_calls:
    print("📊 RPC Call Stats:")
    successful = [e for e in rpc_calls if e.get('success', False)]
    failed = [e for e in rpc_calls if not e.get('success', True)]
    durations = [e.get('duration_ms', 0) for e in rpc_calls]
    avg_duration = sum(durations) / len(durations) if durations else 0
    
    print(f"  Total: {len(rpc_calls)}")
    print(f"  Successful: {len(successful)}")
    print(f"  Failed: {len(failed)}")
    print(f"  Avg duration: {avg_duration:.1f}ms")
    print()

# Discovery cycles
discovery_cycles = [e for e in events if e.get('phase') == 'discovery_cycle']
if discovery_cycles:
    print("📊 Discovery Cycles:")
    durations = [e.get('duration_ms', 0) for e in discovery_cycles]
    avg_duration = sum(durations) / len(durations) if durations else 0
    
    pools_discovered = []
    for e in discovery_cycles:
        result = e.get('result', {})
        pools = result.get('pools_discovered', 0)
        pools_discovered.append(pools)
    
    avg_pools = sum(pools_discovered) / len(pools_discovered) if pools_discovered else 0
    print(f"  Count: {len(discovery_cycles)}")
    print(f"  Avg duration: {avg_duration:.1f}ms")
    print(f"  Avg pools discovered: {avg_pools:.1f}")
    print()

# Hot Pool Manager
hot_pool_events = [e for e in events if e.get('phase') == 'hot_pool_manager_update_weights']
if hot_pool_events:
    print("📊 Hot Pool Manager Updates:")
    durations = [e.get('duration_ms', 0) for e in hot_pool_events]
    avg_duration = sum(durations) / len(durations) if durations else 0
    
    weights_counts = []
    for e in hot_pool_events:
        result = e.get('result', {})
        count = result.get('weights_count', 0)
        weights_counts.append(count)
    
    avg_weights = sum(weights_counts) / len(weights_counts) if weights_counts else 0
    print(f"  Count: {len(hot_pool_events)}")
    print(f"  Avg duration: {avg_duration:.1f}ms")
    print(f"  Avg weights count: {avg_weights:.0f}")
    print()

# Sample weight refresh event
weight_refresh_sample = next((e for e in weight_refresh_events if e.get('phase') == 'weight_refresh_hot'), None)
if not weight_refresh_sample:
    weight_refresh_sample = next((e for e in weight_refresh_events), None)

if weight_refresh_sample:
    print("📊 Sample Weight Refresh Event:")
    print(json.dumps(weight_refresh_sample, indent=2))
    print()
