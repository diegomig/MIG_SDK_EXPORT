// src/utils.rs
// Utility functions for MIG Topology SDK

/// Creates a vector of (start_block, end_block) tuples for a given range and chunk size.
/// Useful for parallelizing block range processing in discovery operations.
pub fn create_block_chunks(from_block: u64, to_block: u64, chunk_size: u64) -> Vec<(u64, u64)> {
    let mut chunks = Vec::new();
    let mut current_from = from_block;
    while current_from <= to_block {
        let current_to = std::cmp::min(current_from + chunk_size - 1, to_block);
        chunks.push((current_from, current_to));
        current_from = current_to + 1;
    }
    chunks
}
