use ethers::prelude::abigen;

abigen!(
    IWETH,
    r#"[
        function deposit() external payable
        function withdraw(uint256 wad) external
        function balanceOf(address guy) external view returns (uint256)
        function transfer(address dst, uint256 wad) external returns (bool)
        function transferFrom(address src, address dst, uint256 wad) external returns (bool)
        function approve(address guy, uint256 wad) external returns (bool)
        function allowance(address src, address guy) external view returns (uint256)
    ]"#
);
