use ethers::prelude::*;

abigen!(
    ICurveAddressProvider,
    r#"[
        function get_registry() external view returns (address)
    ]"#
);
