// SPDX-License-Identifier: MIT
pragma solidity ^0.8.27;

import "forge-std/Script.sol";
import "../src/Arbitraj.sol";

// =====================================================================
//
//  DEPLOY SCRIPT -- ArbitrajBotu v12.0
//  Base Network (Chain ID: 8453)
//
//  v12.0: On-chain whitelist kaldirildi. Havuz dogrulama off-chain'de
//         (Rust bot) yapilir. Kontrat sadece executor + admin ile deploy.
//
//  Kullanim:
//    forge script script/Deploy.s.sol:DeployArbitraj \
//      --rpc-url $BASE_RPC_URL \
//      --broadcast \
//      --verify \
//      --etherscan-api-key $BASESCAN_API_KEY \
//      -vvvv
//
//  Gerekli .env degiskenleri:
//    DEPLOYER_PRIVATE_KEY  -- Deploy eden hesabin private key'i
//    EXECUTOR_ADDRESS      -- Bot executor adresi (sicak cuzdan)
//    ADMIN_ADDRESS         -- Admin adresi (soguk cuzdan / multisig)
//
// =====================================================================

contract DeployArbitraj is Script {
    function run() external {
        uint256 deployerKey = vm.envUint("DEPLOYER_PRIVATE_KEY");
        address executorAddr = vm.envAddress("EXECUTOR_ADDRESS");
        address adminAddr = vm.envAddress("ADMIN_ADDRESS");

        require(executorAddr != address(0), "EXECUTOR_ADDRESS bos");
        require(adminAddr != address(0), "ADMIN_ADDRESS bos");

        console.log("=== ArbitrajBotu v12.0 Deploy ===");
        console.log("  Executor:", executorAddr);
        console.log("  Admin:   ", adminAddr);

        vm.startBroadcast(deployerKey);

        ArbitrajBotu bot = new ArbitrajBotu(executorAddr, adminAddr);
        console.log("  Kontrat: ", address(bot));

        vm.stopBroadcast();

        require(bot.executor() == executorAddr, "Executor hatali");
        require(bot.admin() == adminAddr, "Admin hatali");

        console.log("  Roller dogrulandi (executor + admin immutable)");
        console.log("  Not: Whitelist gerekli degil (off-chain dogrulama)");
        console.log("=== Deploy tamamlandi ===");
    }
}
