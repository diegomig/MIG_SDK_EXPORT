use ethers::prelude::*;

abigen!(
    ICurvePool,
    r#"[
        {
            "name": "A",
            "inputs": [],
            "outputs": [
                {
                    "type": "uint256",
                    "name": ""
                }
            ],
            "stateMutability": "view",
            "type": "function"
        },
        {
            "name": "get_dy",
            "inputs": [
                {
                    "type": "int128",
                    "name": "i"
                },
                {
                    "type": "int128",
                    "name": "j"
                },
                {
                    "type": "uint256",
                    "name": "dx"
                }
            ],
            "outputs": [
                {
                    "type": "uint256",
                    "name": ""
                }
            ],
            "stateMutability": "view",
            "type": "function"
        },
        {
            "name": "coins",
            "inputs": [
                {
                    "type": "uint256",
                    "name": "i"
                }
            ],
            "outputs": [
                {
                    "type": "address",
                    "name": ""
                }
            ],
            "stateMutability": "view",
            "type": "function"
        },
        {
            "name": "balances",
            "inputs": [
                {
                    "type": "uint256",
                    "name": "i"
                }
            ],
            "outputs": [
                {
                    "type": "uint256",
                    "name": ""
                }
            ],
            "stateMutability": "view",
            "type": "function"
        },
        {
            "name": "fee",
            "inputs": [],
            "outputs": [
                {
                    "type": "uint256",
                    "name": ""
                }
            ],
            "stateMutability": "view",
            "type": "function"
        }
    ]"#
);
