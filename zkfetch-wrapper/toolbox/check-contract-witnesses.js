/**
 * Check Contract Witnesses - Proof Witness Lookup Debugger
 * 
 * Purpose: Queries the Sepolia Reclaim contract to see what witnesses
 * it returns for a specific proof using fetchWitnessesForClaim() - the same
 * function the SDK calls during verification.
 * 
 * What it does:
 * - Loads proof from proof-structure.json
 * - Calls fetchWitnessesForClaim(epoch, identifier, timestamp)
 * - Shows which witnesses the contract expects
 * - Recovers actual signer from proof signature
 * - Compares to see if they match
 * 
 * Usage: node toolbox/check-contract-witnesses.js
 * Note: Requires proof-structure.json in current directory
 */

const { ethers } = require('ethers');
const fs = require('fs');
require('dotenv').config();

// Sepolia - the SDK's default network
const RECLAIM_ADDRESS = process.env.RECLAIM_ADDRESS || '0xAe94FB09711e1c6B057853a515483792d8e474d0';
const RPC_URL = process.env.SEPOLIA_RPC_URL;

const ABI = [
    "function fetchWitnessesForClaim(uint32 epoch, bytes32 identifier, uint32 timestampS) view returns (tuple(address addr, string host)[])"
];

async function checkWitnessesUsed() {
    console.log('='.repeat(70));
    console.log('  CHECKING WITNESSES FROM SEPOLIA CONTRACT');
    console.log('='.repeat(70));
    
    const proof = JSON.parse(fs.readFileSync('./proof-structure.json', 'utf8'));
    
    console.log('\n📋 Proof Details:');
    console.log('  Identifier:', proof.identifier);
    console.log('  Epoch:', proof.claimData.epoch);
    console.log('  Timestamp:', proof.claimData.timestampS, '(' + new Date(proof.claimData.timestampS * 1000).toISOString() + ')');
    console.log('  Witness in metadata:', proof.witnesses[0].id);
    
    // Connect to the contract the SDK uses
    console.log('\n📡 Connecting to Sepolia...');
    console.log('  Contract:', RECLAIM_ADDRESS);
    
    const provider = new ethers.JsonRpcProvider(RPC_URL);
    const contract = new ethers.Contract(RECLAIM_ADDRESS, ABI, provider);
    
    try {
        console.log('\n🔍 Calling fetchWitnessesForClaim()...');
        console.log('  This is the same function the SDK calls');
        console.log('');
        
        const witnesses = await contract.fetchWitnessesForClaim(
            proof.claimData.epoch,
            proof.identifier,
            proof.claimData.timestampS
        );
        
        console.log('✅ Retrieved', witnesses.length, 'witness(es):');
        witnesses.forEach((w, i) => {
            console.log(`  [${i}] Address: ${w.addr}`);
            console.log(`      Host: ${w.host}`);
        });
        
        // Recover actual signer using correct method
        console.log('\n🔐 Signature Analysis:');
        const { identifier, owner, timestampS, epoch } = {
            identifier: proof.identifier,
            owner: proof.claimData.owner,
            timestampS: proof.claimData.timestampS,
            epoch: proof.claimData.epoch
        };
        
        const message = identifier.toLowerCase() + '\n' + owner.toLowerCase() + '\n' + timestampS + '\n' + epoch;
        const recoveredSigner = ethers.verifyMessage(message, ethers.hexlify(proof.signatures[0]));
        
        console.log('  Recovered signer:', recoveredSigner.toLowerCase());
        
        // Check if signer matches any expected witness
        console.log('\n📊 Verification Check:');
        const witnessAddresses = witnesses.map(w => w.addr.toLowerCase());
        console.log('  Expected witnesses:', witnessAddresses.join(', '));
        console.log('  Actual signer:', recoveredSigner.toLowerCase());
        
        const signerMatch = witnessAddresses.includes(recoveredSigner.toLowerCase());
        console.log('  Match:', signerMatch ? '✅ YES - Signature is from expected witness!' : '❌ NO - Signature is NOT from expected witness');
        
        if (!signerMatch && witnessAddresses.length === 1 && witnessAddresses[0] === '0x0000000000000000000000000000000000000020') {
            console.log('\n⚠️  The contract has a DUMMY witness (0x...0020)');
            console.log('     This is why on-chain verification fails!');
            console.log('     The SDK must be using a different source for witnesses.');
        }
        
    } catch (error) {
        console.error('\n❌ Error querying contract:', error.message);
        if (error.data) {
            console.error('  Data:', error.data);
        }
    }
    
    console.log('\n' + '='.repeat(70));
}

checkWitnessesUsed().catch(console.error);
