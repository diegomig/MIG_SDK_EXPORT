//! # MIG Topology SDK
//!
//! A high-performance Rust library for real-time liquidity topology mapping and pool validation
//! on Arbitrum One. This SDK provides the infrastructure layer for discovering, normalizing, and
//! validating liquidity pools across multiple DEX protocols.
//!
//! ## Overview
//!
//! The MIG Topology SDK separates the infrastructure layer (discovery, normalization, validation)
//! from execution logic. It focuses on:
//!
//! - **Discovery**: Event-driven pool discovery from blockchain events
//! - **Normalization**: Unified pool representation across DEX protocols
//! - **Validation**: Pool quality assessment and filtering
//! - **Graph Management**: Real-time liquidity topology graph
//!
//! ## Architecture
//!
//! The SDK is organized into several layers:
//!
//! ### Discovery Layer
//! Scans blockchain events (`PairCreated`, `PoolCreated`) to discover new pools in real-time.
//!
//! ### Normalization Layer
//! Standardizes pool data from different DEX protocols (Uniswap, Balancer, Curve, etc.) into
//! a unified representation.
//!
//! ### Validation Layer
//! Validates pool contracts, filters pools without liquidity, and manages blacklists for
//! corrupted or failing pools.
//!
//! ### Graph & State Layer
//! Maintains a weighted liquidity graph with real-time updates and JIT state synchronization.

// Core Types
/// Trait for DEX-specific adapters
pub mod dex_adapter;
/// Unified pool representation across all DEX protocols
pub mod pools;
/// Common types and data structures
pub mod types;

// DEX Adapters
/// Protocol-specific adapters (Uniswap V2/V3, Balancer, Curve, etc.)
pub mod adapters;

// Discovery Layer
/// Block parsing utilities
pub mod block_parser;
/// Block streaming infrastructure
pub mod block_stream;
/// Deferred discovery queue management
pub mod deferred_discovery_queue;
/// Background discovery coordination
pub mod discovery;
/// Discovery result processing
pub mod discovery_result_processor;
/// Main discovery orchestrator
pub mod orchestrator;
/// Pool creation event extraction
pub mod pool_event_extractor;
/// Pool priority classification
pub mod pool_priority_classifier;

// Validation Layer
/// Background pool validation
pub mod background_pool_validator;
/// Data pipeline processing
pub mod data_pipeline;
/// Data quality validation
pub mod data_validator;
/// Data normalization utilities
pub mod normalization;
/// Pool blacklist management
pub mod pool_blacklist;
/// Pool filtering utilities
pub mod pool_filters;
/// Pool validation caching
pub mod pool_validation_cache;
/// Pool validation logic
pub mod validator;

// State & Graph Management
/// Block number caching
pub mod block_number_cache;
/// WebSocket block number subscription
pub mod block_number_websocket;
/// General caching utilities
pub mod cache;
/// Cache state management (Merkle tree-based)
pub mod cache_state;
/// Event indexing and gap detection
pub mod event_indexer;
/// Liquidity graph service with weight calculation
pub mod graph_service;
/// Hot pool manager (in-memory cache)
pub mod hot_pool_manager;
/// Just-In-Time state fetching
pub mod jit_state_fetcher;
/// Route pre-computation and caching
pub mod route_precomputer;
/// Routing primitives (SwapStep, CandidateRoute, etc.)
pub mod router;
/// Unified state fetching for all pool types
pub mod unified_state_fetcher;

// Infrastructure
/// PostgreSQL database integration
pub mod database;
/// Flight recorder for debugging (optional)
pub mod flight_recorder;
/// Metrics and observability
pub mod metrics;
/// Async PostgreSQL writer
pub mod postgres_async_writer;
/// Redis cache manager (optional, feature-gated)
pub mod redis_manager;
/// RPC provider pool with load balancing
pub mod rpc_pool;
/// RPC tracing middleware
pub mod rpc_tracing_middleware;

// Market Health Metadata (Price Feeds)
/// Background price updater
pub mod background_price_updater;
/// CoinGecko price integration
pub mod coingecko_price_updater;
/// External price source integration
pub mod external_price_updater;
/// Price feed aggregation
pub mod price_feeds;
/// Token metadata enrichment
pub mod token_enricher;
/// Weight refresher for historical pool updates
pub mod weight_refresher;

// Utilities
/// Multicall batch RPC utilities
pub mod multicall;
/// General utilities
pub mod utils;
/// Uniswap V3 math utilities
pub mod v3_math;

// Contracts (Public ABIs Only)
/// Smart contract ABIs (read-only, no execution contracts)
pub mod contracts;

// Settings & Configuration
/// Configuration management
pub mod settings;

// Re-exports for convenience
pub use dex_adapter::DexAdapter;
pub use graph_service::GraphService;
pub use orchestrator::Orchestrator;
pub use pools::Pool;
pub use settings::Settings;
pub use validator::PoolValidator;
