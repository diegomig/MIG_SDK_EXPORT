# Metrics and Success Criteria

## Target Metrics

### Latency

- **End-to-End Latency (p95)**: <50ms
- **RPC Latency (local node)**: <10ms
- **Block Number Update Latency**: <50ms from new block

### Cache Performance

- **JIT Cache Hit Rate**: >90%
- **Price Feed Cache Hit Rate**: >90%
- **False Invalidation Rate**: <0.1%

### RPC Efficiency

- **RPC Calls per Block**: <5 (with local node)
- **Multicall Success Rate**: >95%
- **Circuit Breaker Open Rate**: <1%

### System Reliability

- **Uptime**: >99.9%
- **WebSocket Connection Uptime**: >99%
- **Polling Fallback Activation**: <1% of time

### Database Performance

- **Write Batch Size (avg)**: 500-1000 items
- **Checkpoint Latency**: <50ms
- **Connection Pool Usage**: <80% capacity (with PgBouncer: <50%)

### Data Quality

- **Gap Detection Accuracy**: 100% (all gaps detected)
- **Price Feed Success Rate**: >95%
- **Zero Price Rate**: <0.1%

## Monitoring

### Key Metrics to Track

1. **RPC Latency**: Monitor `rpc_latency_ms` histogram
2. **Cache Hit Rate**: Monitor `cache_hits` vs `cache_misses`
3. **Multicall Success Rate**: Monitor `multicall_success` vs `multicall_failures`
4. **Circuit Breaker State**: Monitor `circuit_breaker_state` gauge
5. **Database Batch Size**: Monitor `db_batch_size` histogram
6. **Block Number Update Latency**: Monitor time from block creation to cache update

### Alerts

Set up alerts for:
- RPC latency p95 >100ms
- Cache hit rate <80%
- Circuit breaker open >5% of time
- Database connection pool usage >90%
- Gap detection finds >10 missing blocks

## Performance Tuning

### If Latency is High

1. Ensure local node is configured and healthy
2. Check WebSocket connection status
3. Verify cache hit rates (should be >90%)
4. Review multicall batch sizes (should be 80-100 calls)

### If Cache Hit Rate is Low

1. Check Merkle cache TTL settings
2. Verify block number cache is updating correctly
3. Review cache invalidation logic (should only invalidate on state change)

### If RPC Calls are High

1. Verify local node is being used (check logs for "Local node" messages)
2. Check multicall batching is working (should see batch sizes >50)
3. Review cache hit rates (low hit rate = more RPC calls)

