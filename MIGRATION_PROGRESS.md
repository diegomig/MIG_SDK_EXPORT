# Schema Migration Progress

## Status: IN PROGRESS

### Completed:
- ✅ Constante SCHEMA definida
- ✅ CREATE SCHEMA cambiado
- ✅ INFORMATION_SCHEMA query cambiado
- ✅ INSERT INTO configurations cambiado
- ✅ create_tables() función completa (todas las CREATE TABLE y ALTER TABLE)

### Remaining in database.rs:
- get_dex_state()
- set_dex_state()
- upsert_token()
- batch_upsert_tokens()
- upsert_token_relation()
- update_token_oracle_source()
- get_tokens_without_symbols()
- get_tokens_needing_enrichment()
- upsert_pool()
- insert_pool_snapshot()
- upsert_graph_weight()
- load_all_graph_weights()
- load_valid_pools_with_weights()
- Y más...

### Next steps:
Continue systematic replacement of all queries in database.rs
