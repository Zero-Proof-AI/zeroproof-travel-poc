// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "../src/ZeroProof.sol";

contract DeployZeroProof is Script {
    function run() external {
        // Get deployer private key from environment
        uint256 deployerPrivateKey = vm.envUint("PRIVATE_KEY");
        
        // Get verifier addresses from environment or use defaults
        address sp1Verifier = vm.envOr(
            "SP1_VERIFIER_ADDRESS",
            address(0x53A9038dCB210D210A7C973fA066Fd2C50aa8847) // Sepolia SP1 v5.2.4 Groth16
        );
        
        address reclaimVerifier = vm.envOr(
            "RECLAIM_VERIFIER_ADDRESS",
            address(0x0000000000000000000000000000000000000000) // TODO: Deploy Reclaim verifier first
        );

        console.log("Deploying ZeroProof with:");
        console.log("  SP1 Verifier:", sp1Verifier);
        console.log("  Reclaim Verifier:", reclaimVerifier);

        vm.startBroadcast(deployerPrivateKey);

        ZeroProof zeroProof = new ZeroProof(sp1Verifier, reclaimVerifier);

        vm.stopBroadcast();

        console.log("\n=== Deployment Complete ===");
        console.log("ZeroProof deployed at:", address(zeroProof));
        console.log("\nVerify on Etherscan:");
        console.log("  forge verify-contract");
        console.log("  --chain-id");
        
        console.log("\nUpdate Agent A environment:");
        console.log("  export ZEROPROOF_ADDRESS=");
        console.logAddress(address(zeroProof));
    }
}
