use ethers::prelude::*;

abigen!(
    ICurveRegistry,
    r#"[
        function pool_count() external view returns (uint256)
        function pool_list(uint256) external view returns (address)
        function get_pool_from_lp_token(address) external view returns (address)
        function get_gauges(address) external view returns (address[10], int128[10])
    ]"#
);
