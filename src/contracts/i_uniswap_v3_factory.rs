use ethers::prelude::*;

abigen!(
    IUniswapV3Factory,
    r#"[
        event PoolCreated(address indexed token0, address indexed token1, uint24 indexed fee, int24 tickSpacing, address pool)
        function getPool(address tokenA, address tokenB, uint24 fee) external view returns (address pool)
    ]"#
);
