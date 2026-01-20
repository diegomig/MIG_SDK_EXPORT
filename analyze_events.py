#!/usr/bin/env python3
import json
import sys
from collections import defaultdict

file_path = "benchmarks/flight_recorder_20260118_014659.jsonl"

print("=" * 70)
print("📊 ANÁLISIS DETALLADO DE EVENTOS")
print("=" * 70)
print()

events = []
with open(file_path, 'r') as f:
    for line in f:
        if line.strip():
            try:
                events.append(json.loads(line))
            except:
                continue

print(f"Total events: {len(events)}\n")

# Phases
phases = [e for e in events if e.get('type') in ['PhaseStart', 'PhaseEnd']]
print("📊 PHASES ENCONTRADAS:")
for e in phases:
    phase_type = e.get('type')
    phase_name = e.get('phase', 'unknown')
    if phase_type == 'PhaseStart':
        print(f"  START: {phase_name}")
        metadata = e.get('metadata', {})
        if metadata:
            print(f"    Metadata: {json.dumps(metadata, indent=4)}")
    elif phase_type == 'PhaseEnd':
        duration = e.get('duration_ms', 0)
        result = e.get('result', {})
        print(f"  END: {phase_name} (duration: {duration}ms)")
        if result:
            print(f"    Result: {json.dumps(result, indent=4)}")
    print()

print("\n" + "=" * 70)
print("📊 SECUENCIA DE EVENTOS CLAVE")
print("=" * 70)

# Find discovery -> graph update sequences
for i in range(len(events) - 1):
    e1 = events[i]
    e2 = events[i + 1]
    
    if (e1.get('phase') == 'discovery_cycle' and e1.get('type') == 'PhaseEnd' and
        e2.get('phase') == 'graph_updates' and e2.get('type') == 'PhaseStart'):
        print(f"\n✅ Secuencia encontrada: Discovery → Graph Update")
        print(f"  Discovery end: {e1.get('duration_ms', 0)}ms")
        print(f"  Graph update start metadata: {json.dumps(e2.get('metadata', {}), indent=4)}")

print()
