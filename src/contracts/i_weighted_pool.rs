use ethers::prelude::*;

abigen!(
    IWeightedPool,
    r#"[
        function getPoolId() external view returns (bytes32)
        function getSwapFeePercentage() external view returns (uint256)
    ]"#
);
