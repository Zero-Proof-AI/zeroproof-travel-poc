/**
 * Full On-Chain Verification Test
 * Generates fresh proof and performs actual on-chain verification
 * using the Reclaim contract on Ethereum Sepolia
 */

const { verifyProof, transformForOnchain } = require('@reclaimprotocol/js-sdk');
const { generateProof } = require('./generate-proof');
const { ethers } = require('ethers');
const fs = require('fs');
const path = require('path');
require('dotenv').config();

// Ethereum Sepolia configuration
const SEPOLIA_RPC = process.env.SEPOLIA_RPC_URL;
const RECLAIM_ADDRESS = process.env.RECLAIM_ADDRESS || '0xAe94FB09711e1c6B057853a515483792d8e474d0';
const PRIVATE_KEY = process.env.PRIVATE_KEY; // Add private key for signing transactions

// Minimal ABI for verifyProof function
const RECLAIM_ABI = [
    {
        "inputs": [
            {
                "components": [
                    {
                        "components": [
                            { "internalType": "string", "name": "provider", "type": "string" },
                            { "internalType": "string", "name": "parameters", "type": "string" },
                            { "internalType": "string", "name": "context", "type": "string" }
                        ],
                        "internalType": "struct Claims.ClaimInfo",
                        "name": "claimInfo",
                        "type": "tuple"
                    },
                    {
                        "components": [
                            {
                                "components": [
                                    { "internalType": "bytes32", "name": "identifier", "type": "bytes32" },
                                    { "internalType": "address", "name": "owner", "type": "address" },
                                    { "internalType": "uint32", "name": "timestampS", "type": "uint32" },
                                    { "internalType": "uint32", "name": "epoch", "type": "uint32" }
                                ],
                                "internalType": "struct Claims.CompleteClaimData",
                                "name": "claim",
                                "type": "tuple"
                            },
                            { "internalType": "bytes[]", "name": "signatures", "type": "bytes[]" }
                        ],
                        "internalType": "struct Claims.SignedClaim",
                        "name": "signedClaim",
                        "type": "tuple"
                    }
                ],
                "internalType": "struct Reclaim.Proof",
                "name": "proof",
                "type": "tuple"
            }
        ],
        "name": "verifyProof",
        "outputs": [{ "internalType": "bool", "name": "", "type": "bool" }],
        "stateMutability": "nonpayable",
        "type": "function"
    }
];

async function testOnChainVerification() {
    console.log('\n========================================');
    console.log('🔗 FULL ON-CHAIN VERIFICATION TEST');
    console.log('========================================\n');

    // Check configuration
    if (!SEPOLIA_RPC) {
        console.error('❌ SEPOLIA_RPC_URL not configured in .env');
        console.error('   Please set SEPOLIA_RPC_URL=https://rpc.ankr.com/eth_sepolia/...');
        process.exit(1);
    }

    if (!PRIVATE_KEY) {
        console.error('❌ PRIVATE_KEY not configured in .env');
        console.error('   Please set PRIVATE_KEY=0x... (with Sepolia ETH for gas)');
        console.error('   Get test ETH from https://sepoliafaucet.com/');
        process.exit(1);
    }

    try {
        // Step 1: Generate fresh proof
        console.log('📝 Step 1: Generating fresh proof...');
        await generateProof();

        const proof = JSON.parse(fs.readFileSync('proof-structure.json', 'utf8'));
        console.log('✅ Proof generated!');
        console.log('   Proof Identifier:', proof.identifier.substring(0, 50) + '...');
        console.log('   Timestamp:', new Date(proof.claimData.timestampS * 1000).toISOString());

        // Step 2: Verify off-chain first
        console.log('\n📝 Step 2: Verifying proof off-chain...');
        const isValid = await verifyProof(proof);

        if (!isValid) {
            throw new Error('❌ Proof failed off-chain verification - cannot proceed to on-chain test');
        }
        console.log('✅ Off-chain verification PASSED!');

        // Step 3: Transform for on-chain
        console.log('\n📝 Step 3: Transforming proof for on-chain...');
        const onchainProof = transformForOnchain(proof);

        console.log('✅ Proof transformed for on-chain use!');
        console.log('   Claim Info Provider:', onchainProof.claimInfo.provider);
        console.log('   Signed Claim Epoch:', onchainProof.signedClaim.claim.epoch);
        console.log('   Signatures Count:', onchainProof.signedClaim.signatures.length);

        // Step 4: Connect to Ethereum Sepolia
        console.log('\n📝 Step 4: Connecting to Ethereum Sepolia...');
        console.log('   RPC URL:', SEPOLIA_RPC.substring(0, 50) + '...');
        console.log('   Contract:', RECLAIM_ADDRESS);

        const provider = new ethers.JsonRpcProvider(SEPOLIA_RPC);
        const network = await provider.getNetwork();
        console.log('   ✅ Connected to network:', network.name, '(Chain ID:', network.chainId.toString() + ')');

        // Step 4.5: Create signer
        console.log('\n📝 Step 4.5: Creating transaction signer...');
        const signer = new ethers.Wallet(PRIVATE_KEY, provider);
        const address = await signer.getAddress();
        console.log('   ✅ Signer created for address:', address);

        // Check balance
        const balance = await provider.getBalance(address);
        console.log('   💰 Account balance:', ethers.formatEther(balance), 'ETH');
        if (balance < ethers.parseEther('0.01')) {
            console.error('   ❌ Insufficient balance for gas fees');
            console.error('   ℹ️  Get test ETH from https://sepoliafaucet.com/');
            process.exit(1);
        }

        // Step 5: Create contract instance
        console.log('\n📝 Step 5: Creating Reclaim contract instance...');
        const reclaimContract = new ethers.Contract(RECLAIM_ADDRESS, RECLAIM_ABI, signer);
        console.log('   ✅ Contract instance created with signer');

        // Step 6: Verify contract exists
        console.log('\n📝 Step 6: Verifying contract deployment...');
        const code = await provider.getCode(RECLAIM_ADDRESS);
        if (code === '0x') {
            throw new Error('❌ Contract not deployed at address: ' + RECLAIM_ADDRESS);
        }
        console.log('   ✅ Contract deployed and accessible');

        // Step 7: Estimate gas first
        console.log('\n📝 Step 7: Estimating gas for on-chain verification...');
        const gasEstimate = await reclaimContract.verifyProof.estimateGas(onchainProof);
        console.log('   📊 Estimated gas:', gasEstimate.toString());
        console.log('   💰 Est. cost (at 20 gwei):', ethers.formatEther(gasEstimate * 20n * ethers.parseUnits('1', 'gwei')), 'ETH');

        // Step 8: Execute actual on-chain verification
        console.log('\n📝 Step 8: Executing on-chain verification...');
        console.log('   🔄 Calling verifyProof() on Reclaim contract...');

        const startTime = Date.now();
        const tx = await reclaimContract.verifyProof(onchainProof);
        const endTime = Date.now();

        console.log('   ✅ Transaction submitted!');
        console.log('   📋 Transaction Hash:', tx.hash);
        console.log('   ⏱️  Execution time:', (endTime - startTime), 'ms');

        // Wait for transaction confirmation
        console.log('\n📝 Step 9: Waiting for transaction confirmation...');
        const receipt = await tx.wait();
        console.log('   ✅ Transaction confirmed!');
        console.log('   📋 Block Number:', receipt.blockNumber);
        console.log('   ⛽ Gas Used:', receipt.gasUsed.toString());
        console.log('   💰 Actual Cost:', ethers.formatEther(receipt.gasUsed * receipt.gasPrice), 'ETH');

        // Parse the result - for nonpayable functions, we need to check if it reverted
        let verified = true; // Assume success unless reverted
        if (receipt.status === 0) {
            verified = false; // Transaction reverted
        }

        console.log('\n📝 Step 10: Verification Result:');
        console.log('   ─────────────────────────────────────────────────');
        if (verified) {
            console.log('   ✅ ON-CHAIN VERIFICATION: SUCCESS!');
            console.log('   🎉 Proof cryptographically verified on Ethereum!');
            console.log('   🔒 Zero-knowledge proof validated by smart contract');
        } else {
            console.log('   ❌ ON-CHAIN VERIFICATION: FAILED!');
            console.log('   ⚠️  Transaction reverted - proof rejected by Reclaim contract');
        }

        // Summary
        console.log('\n========================================');
        console.log('🎯 ON-CHAIN VERIFICATION TEST COMPLETE');
        console.log('========================================');
        console.log('');
        console.log('📊 Test Results:');
        console.log('  ✅ Proof generated from httpbin.org');
        console.log('  ✅ Off-chain verification successful');
        console.log('  ✅ Proof transformed for on-chain');
        console.log('  ✅ Contract interaction successful');
        console.log('  ✅ Transaction executed and confirmed');
        console.log('  ', verified ? '✅' : '❌', 'On-chain verification:', verified ? 'PASSED' : 'FAILED');
        console.log('');
        console.log('🔗 Blockchain Details:');
        console.log('  Network: Ethereum Sepolia');
        console.log('  Contract:', RECLAIM_ADDRESS);
        console.log('  Transaction:', tx.hash);
        console.log('  Block:', receipt.blockNumber);
        console.log('');
        console.log('🎉 End-to-end zero-knowledge proof verification completed!');

        if (verified) {
            console.log('\n🏆 SUCCESS: Your zkTLS proof was verified on-chain!');
            console.log('   This proves the HTTP request was made without revealing sensitive data.');
        } else {
            console.log('\n⚠️  FAILURE: Proof verification failed on-chain.');
            console.log('   Check: proof validity, contract state, network issues.');
        }

        process.exit(verified ? 0 : 1);

    } catch (error) {
        console.error('\n❌ On-chain verification test failed:', error.message);

        if (error.code === 'NETWORK_ERROR') {
            console.error('   ℹ️  Network connectivity issue');
        } else if (error.code === 'CALL_EXCEPTION') {
            console.error('   ℹ️  Contract call reverted - check proof validity');
        } else if (error.code === 'INSUFFICIENT_FUNDS') {
            console.error('   ℹ️  Insufficient funds for transaction');
        } else if (error.code === 'NONCE_EXPIRED') {
            console.error('   ℹ️  Nonce expired - retry the transaction');
        }

        console.error('\n🔍 Troubleshooting:');
        console.error('  1. Check SEPOLIA_RPC_URL is correct and accessible');
        console.error('  2. Verify PRIVATE_KEY is set and has Sepolia ETH');
        console.error('  3. Verify RECLAIM_ADDRESS is deployed on Sepolia');
        console.error('  4. Ensure proof is valid (passes off-chain verification)');
        console.error('  5. Check network congestion and gas prices');
        console.error('  6. Get test ETH from https://sepoliafaucet.com/ if needed');

        process.exit(1);
    }
}

// Run the test
if (require.main === module) {
    testOnChainVerification().catch(err => {
        console.error('Fatal error:', err.message);
        process.exit(1);
    });
}

module.exports = { testOnChainVerification };