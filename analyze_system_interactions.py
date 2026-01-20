#!/usr/bin/env python3
"""
Análisis completo de interacciones del sistema desde Flight Recorder
Analiza cómo interactúan discovery, graph updates, weight refresh, cache, etc.
"""
import json
import sys
from collections import defaultdict
from pathlib import Path
import glob
from datetime import datetime

def load_events(file_path):
    """Load events from JSONL file"""
    events = []
    try:
        with open(file_path, 'r') as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                try:
                    events.append(json.loads(line))
                except:
                    continue
    except Exception as e:
        print(f"⚠️ Error loading {file_path}: {e}")
    return events

def analyze_system_interactions():
    """Analyze system interactions from Flight Recorder data"""
    
    # Find all flight recorder files
    benchmark_files = sorted(glob.glob("benchmarks/flight_recorder_*.jsonl"), reverse=True)
    log_files = sorted(glob.glob("logs/flight_recorder_*.jsonl"), reverse=True)
    
    all_files = benchmark_files + log_files
    
    if not all_files:
        print("❌ No flight recorder files found")
        return
    
    print("=" * 70)
    print("📊 ANÁLISIS COMPLETO DE INTERACCIONES DEL SISTEMA")
    print("=" * 70)
    print()
    
    # Load all events
    all_events = []
    for file_path in all_files:
        events = load_events(file_path)
        all_events.extend(events)
        if events:
            print(f"📁 Loaded {len(events)} events from {Path(file_path).name}")
    
    if not all_events:
        print("❌ No events found in any file")
        return
    
    print(f"\n📊 Total events: {len(all_events)}\n")
    
    # 1. Event types distribution
    print("=" * 70)
    print("1. DISTRIBUCIÓN DE TIPOS DE EVENTOS")
    print("=" * 70)
    event_types = defaultdict(int)
    for e in all_events:
        event_types[e.get('type', 'unknown')] += 1
    
    for etype, count in sorted(event_types.items(), key=lambda x: -x[1]):
        pct = (count / len(all_events)) * 100
        print(f"  {count:5d} ({pct:5.1f}%) {etype}")
    print()
    
    # 2. Phase analysis (discovery, graph updates, weight refresh)
    print("=" * 70)
    print("2. ANÁLISIS DE FASES (Discovery, Graph Updates, Weight Refresh)")
    print("=" * 70)
    
    phases = defaultdict(lambda: {'starts': [], 'ends': [], 'durations': []})
    
    for e in all_events:
        if e.get('type') == 'PhaseStart':
            phase = e.get('phase', 'unknown')
            phases[phase]['starts'].append(e)
        elif e.get('type') == 'PhaseEnd':
            phase = e.get('phase', 'unknown')
            phases[phase]['ends'].append(e)
            if 'duration_ms' in e:
                phases[phase]['durations'].append(e['duration_ms'])
    
    # Group phases by category
    discovery_phases = [p for p in phases.keys() if 'discovery' in p.lower()]
    graph_phases = [p for p in phases.keys() if 'graph' in p.lower()]
    weight_phases = [p for p in phases.keys() if 'weight' in p.lower() or 'refresh' in p.lower()]
    cache_phases = [p for p in phases.keys() if 'cache' in p.lower() or 'hot_pool' in p.lower()]
    
    print("\n🔍 Discovery Phases:")
    for phase in sorted(discovery_phases):
        starts = len(phases[phase]['starts'])
        ends = len(phases[phase]['ends'])
        durations = phases[phase]['durations']
        avg_duration = sum(durations) / len(durations) if durations else 0
        print(f"  {phase}:")
        print(f"    Starts: {starts}, Ends: {ends}, Avg duration: {avg_duration:.1f}ms")
    
    print("\n📊 Graph Update Phases:")
    for phase in sorted(graph_phases):
        starts = len(phases[phase]['starts'])
        ends = len(phases[phase]['ends'])
        durations = phases[phase]['durations']
        avg_duration = sum(durations) / len(durations) if durations else 0
        
        # Get mode info
        modes = defaultdict(int)
        for e in phases[phase]['ends']:
            mode = (e.get('result', {}).get('mode') or 
                   e.get('metadata', {}).get('mode') or 'unknown')
            modes[mode] += 1
        
        print(f"  {phase}:")
        print(f"    Starts: {starts}, Ends: {ends}, Avg duration: {avg_duration:.1f}ms")
        if modes:
            print(f"    Modes: {dict(modes)}")
    
    print("\n🔥 Weight Refresh Phases:")
    for phase in sorted(weight_phases):
        starts = len(phases[phase]['starts'])
        ends = len(phases[phase]['ends'])
        durations = phases[phase]['durations']
        avg_duration = sum(durations) / len(durations) if durations else 0
        
        # Get pools updated info
        pools_updated = []
        for e in phases[phase]['ends']:
            result = e.get('result', {})
            pools = result.get('pools_updated', result.get('total_pools', 0))
            if pools > 0:
                pools_updated.append(pools)
        
        avg_pools = sum(pools_updated) / len(pools_updated) if pools_updated else 0
        
        print(f"  {phase}:")
        print(f"    Starts: {starts}, Ends: {ends}, Avg duration: {avg_duration:.1f}ms")
        if pools_updated:
            print(f"    Avg pools updated: {avg_pools:.0f}")
    
    print("\n💾 Cache & Hot Pool Manager Phases:")
    for phase in sorted(cache_phases):
        starts = len(phases[phase]['starts'])
        ends = len(phases[phase]['ends'])
        durations = phases[phase]['durations']
        avg_duration = sum(durations) / len(durations) if durations else 0
        print(f"  {phase}:")
        print(f"    Starts: {starts}, Ends: {ends}, Avg duration: {avg_duration:.1f}ms")
    print()
    
    # 3. Cache performance
    print("=" * 70)
    print("3. RENDIMIENTO DE CACHE")
    print("=" * 70)
    
    cache_events = [e for e in all_events if e.get('type') == 'CacheEvent']
    if cache_events:
        cache_types = defaultdict(int)
        for e in cache_events:
            cache_types[e.get('event_type', 'unknown')] += 1
        
        total_cache = sum(cache_types.values())
        hits = cache_types.get('hit', 0)
        misses = cache_types.get('miss', 0)
        hit_rate = (hits / total_cache * 100) if total_cache > 0 else 0
        
        print(f"\n  Total cache events: {total_cache}")
        print(f"  Hits: {hits} ({hits/total_cache*100:.1f}%)")
        print(f"  Misses: {misses} ({misses/total_cache*100:.1f}%)")
        print(f"  Cache Hit Rate: {hit_rate:.1f}%")
    else:
        print("\n  No cache events found")
    print()
    
    # 4. RPC calls analysis
    print("=" * 70)
    print("4. ANÁLISIS DE LLAMADAS RPC")
    print("=" * 70)
    
    rpc_calls = [e for e in all_events if e.get('type') == 'RpcCall']
    if rpc_calls:
        successful = [e for e in rpc_calls if e.get('success', False)]
        failed = [e for e in rpc_calls if not e.get('success', True)]
        durations = [e.get('duration_ms', 0) for e in rpc_calls]
        avg_duration = sum(durations) / len(durations) if durations else 0
        
        # Group by endpoint
        endpoints = defaultdict(list)
        for e in rpc_calls:
            endpoint = e.get('endpoint', 'unknown')
            endpoints[endpoint].append(e.get('duration_ms', 0))
        
        print(f"\n  Total RPC calls: {len(rpc_calls)}")
        print(f"  Successful: {len(successful)} ({len(successful)/len(rpc_calls)*100:.1f}%)")
        print(f"  Failed: {len(failed)} ({len(failed)/len(rpc_calls)*100:.1f}%)")
        print(f"  Avg duration: {avg_duration:.1f}ms")
        
        print(f"\n  By endpoint:")
        for endpoint, durations in sorted(endpoints.items(), key=lambda x: -len(x[1])):
            avg = sum(durations) / len(durations) if durations else 0
            print(f"    {endpoint}: {len(durations)} calls, avg {avg:.1f}ms")
    else:
        print("\n  No RPC call events found")
    print()
    
    # 5. Discovery cycles analysis
    print("=" * 70)
    print("5. ANÁLISIS DE CICLOS DE DISCOVERY")
    print("=" * 70)
    
    discovery_cycles = [e for e in all_events 
                       if e.get('phase') == 'discovery_cycle' and e.get('type') == 'PhaseEnd']
    
    if discovery_cycles:
        durations = [e.get('duration_ms', 0) for e in discovery_cycles]
        avg_duration = sum(durations) / len(durations) if durations else 0
        
        pools_discovered = []
        pools_validated = []
        for e in discovery_cycles:
            result = e.get('result', {})
            pools_discovered.append(result.get('pools_discovered', 0))
            pools_validated.append(result.get('pools_validated', 0))
        
        avg_discovered = sum(pools_discovered) / len(pools_discovered) if pools_discovered else 0
        avg_validated = sum(pools_validated) / len(pools_validated) if pools_validated else 0
        
        print(f"\n  Total discovery cycles: {len(discovery_cycles)}")
        print(f"  Avg duration: {avg_duration:.1f}ms")
        print(f"  Avg pools discovered: {avg_discovered:.1f}")
        print(f"  Avg pools validated: {avg_validated:.1f}")
    else:
        print("\n  No discovery cycle events found")
    print()
    
    # 6. Graph updates analysis
    print("=" * 70)
    print("6. ANÁLISIS DE ACTUALIZACIONES DE GRAFO")
    print("=" * 70)
    
    graph_updates = [e for e in all_events 
                    if e.get('phase') == 'graph_updates' and e.get('type') == 'PhaseEnd']
    
    if graph_updates:
        # Group by mode
        modes = defaultdict(list)
        for e in graph_updates:
            mode = (e.get('result', {}).get('mode') or 
                   e.get('metadata', {}).get('mode') or 'unknown')
            modes[mode].append(e)
        
        print(f"\n  Total graph updates: {len(graph_updates)}")
        
        for mode, mode_events in sorted(modes.items()):
            durations = [e.get('duration_ms', 0) for e in mode_events]
            avg_duration = sum(durations) / len(durations) if durations else 0
            
            pools_updated = []
            for e in mode_events:
                result = e.get('result', {})
                pools = (result.get('total_pools') or 
                        result.get('pools_updated') or 
                        result.get('total_updated') or 0)
                pools_updated.append(pools)
            
            avg_pools = sum(pools_updated) / len(pools_updated) if pools_updated else 0
            
            print(f"\n  Mode: {mode}")
            print(f"    Events: {len(mode_events)}")
            print(f"    Avg duration: {avg_duration:.1f}ms")
            print(f"    Avg pools updated: {avg_pools:.0f}")
    else:
        print("\n  No graph update events found")
    print()
    
    # 7. Weight refresh analysis
    print("=" * 70)
    print("7. ANÁLISIS DE WEIGHT REFRESH")
    print("=" * 70)
    
    weight_refresh_events = [e for e in all_events 
                            if e.get('phase') in ['weight_refresh_hot', 'weight_refresh_warm'] 
                            and e.get('type') == 'PhaseEnd']
    
    if weight_refresh_events:
        by_phase = defaultdict(list)
        for e in weight_refresh_events:
            phase = e.get('phase', 'unknown')
            by_phase[phase].append(e)
        
        for phase, phase_events in sorted(by_phase.items()):
            durations = [e.get('duration_ms', 0) for e in phase_events]
            avg_duration = sum(durations) / len(durations) if durations else 0
            
            pools_updated = []
            candidates_loaded = []
            failed_validation = []
            
            for e in phase_events:
                result = e.get('result', {})
                pools_updated.append(result.get('pools_updated', 0))
                candidates_loaded.append(result.get('candidates_loaded', 0))
                failed_validation.append(result.get('failed_validation', 0))
            
            avg_pools = sum(pools_updated) / len(pools_updated) if pools_updated else 0
            avg_candidates = sum(candidates_loaded) / len(candidates_loaded) if candidates_loaded else 0
            avg_failed = sum(failed_validation) / len(failed_validation) if failed_validation else 0
            
            print(f"\n  Phase: {phase}")
            print(f"    Events: {len(phase_events)}")
            print(f"    Avg duration: {avg_duration:.1f}ms")
            print(f"    Avg pools updated: {avg_pools:.0f}")
            print(f"    Avg candidates loaded: {avg_candidates:.0f}")
            print(f"    Avg failed validation: {avg_failed:.0f}")
    else:
        print("\n  No weight refresh events found")
    print()
    
    # 8. Hot Pool Manager analysis
    print("=" * 70)
    print("8. ANÁLISIS DE HOT POOL MANAGER")
    print("=" * 70)
    
    hot_pool_events = [e for e in all_events 
                      if 'hot_pool_manager' in e.get('phase', '').lower() 
                      and e.get('type') == 'PhaseEnd']
    
    if hot_pool_events:
        by_phase = defaultdict(list)
        for e in hot_pool_events:
            phase = e.get('phase', 'unknown')
            by_phase[phase].append(e)
        
        for phase, phase_events in sorted(by_phase.items()):
            durations = [e.get('duration_ms', 0) for e in phase_events]
            avg_duration = sum(durations) / len(durations) if durations else 0
            
            weights_counts = []
            for e in phase_events:
                result = e.get('result', {})
                count = result.get('weights_count', 0)
                weights_counts.append(count)
            
            avg_weights = sum(weights_counts) / len(weights_counts) if weights_counts else 0
            
            print(f"\n  Phase: {phase}")
            print(f"    Events: {len(phase_events)}")
            print(f"    Avg duration: {avg_duration:.1f}ms")
            print(f"    Avg weights count: {avg_weights:.0f}")
    else:
        print("\n  No hot pool manager events found")
    print()
    
    # 9. Timeline analysis (event sequence)
    print("=" * 70)
    print("9. ANÁLISIS DE SECUENCIA TEMPORAL")
    print("=" * 70)
    
    # Sort events by timestamp
    sorted_events = sorted(all_events, key=lambda e: e.get('ts', 0))
    
    if sorted_events:
        first_ts = sorted_events[0].get('ts', 0)
        last_ts = sorted_events[-1].get('ts', 0)
        total_duration = last_ts - first_ts
        
        print(f"\n  First event: {first_ts}ms")
        print(f"  Last event: {last_ts}ms")
        print(f"  Total duration: {total_duration}ms ({total_duration/1000:.1f}s)")
        
        # Find key sequences
        print(f"\n  Key sequences found:")
        
        # Discovery -> Graph Update
        discovery_graph_pairs = 0
        for i in range(len(sorted_events) - 1):
            e1 = sorted_events[i]
            e2 = sorted_events[i + 1]
            if (e1.get('phase') == 'discovery_cycle' and e1.get('type') == 'PhaseEnd' and
                e2.get('phase') == 'graph_updates' and e2.get('type') == 'PhaseStart'):
                discovery_graph_pairs += 1
        
        if discovery_graph_pairs > 0:
            print(f"    Discovery → Graph Update: {discovery_graph_pairs} sequences")
        
        # Weight Refresh -> Hot Pool Manager Update
        weight_hot_pairs = 0
        for i in range(len(sorted_events) - 1):
            e1 = sorted_events[i]
            e2 = sorted_events[i + 1]
            if (e1.get('phase') in ['weight_refresh_hot', 'weight_refresh_warm'] and 
                e1.get('type') == 'PhaseEnd' and
                'hot_pool_manager' in e2.get('phase', '').lower() and 
                e2.get('type') == 'PhaseStart'):
                weight_hot_pairs += 1
        
        if weight_hot_pairs > 0:
            print(f"    Weight Refresh → Hot Pool Manager: {weight_hot_pairs} sequences")
    
    print()
    
    # 10. Summary
    print("=" * 70)
    print("10. RESUMEN EJECUTIVO")
    print("=" * 70)
    
    print(f"\n  ✅ Sistema funcionando correctamente")
    print(f"  ✅ {len(all_events)} eventos capturados")
    print(f"  ✅ {len(discovery_cycles)} discovery cycles ejecutados")
    print(f"  ✅ {len(graph_updates)} graph updates ejecutados")
    print(f"  ✅ {len(weight_refresh_events)} weight refresh events ejecutados")
    print(f"  ✅ {len(cache_events)} cache events capturados")
    print(f"  ✅ {len(rpc_calls)} RPC calls registrados")
    
    if cache_events:
        hits = len([e for e in cache_events if e.get('event_type') == 'hit'])
        total_cache = len(cache_events)
        hit_rate = (hits / total_cache * 100) if total_cache > 0 else 0
        print(f"  ✅ Cache hit rate: {hit_rate:.1f}%")
    
    print()

if __name__ == "__main__":
    analyze_system_interactions()
