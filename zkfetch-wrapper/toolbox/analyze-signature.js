const fs = require('fs');
const { ethers } = require('ethers');

const proof = JSON.parse(fs.readFileSync('proof-structure.json', 'utf8'));

console.log('='.repeat(70));
console.log('  PROOF STRUCTURE ANALYSIS');
console.log('='.repeat(70));

console.log('\n📋 CLAIM DATA BREAKDOWN:\n');
console.log('Provider:', proof.claimData.provider);
console.log('Owner:', proof.claimData.owner);
console.log('Timestamp:', proof.claimData.timestampS, '(' + new Date(proof.claimData.timestampS * 1000).toISOString() + ')');
console.log('Epoch:', proof.claimData.epoch);
console.log('Identifier:', proof.claimData.identifier);

console.log('\n📋 PARAMETERS (parsed):');
const params = JSON.parse(proof.claimData.parameters);
console.log(JSON.stringify(params, null, 2));

console.log('\n📋 CONTEXT (parsed):');
const context = JSON.parse(proof.claimData.context);
console.log(JSON.stringify(context, null, 2));

console.log('\n📋 SIGNATURE VERIFICATION:\n');

// Recreate the message that was signed (using SDK's createSignDataForClaim)
const message = 
    proof.claimData.identifier.toLowerCase() + '\n' +
    proof.claimData.owner.toLowerCase() + '\n' +
    proof.claimData.timestampS + '\n' +
    proof.claimData.epoch;

console.log('Message to sign (createSignDataForClaim format):');
console.log(message);
console.log('');

// Recover signer using ethers.verifyMessage (same as SDK)
const signature = proof.signatures[0];
console.log('Signature:', signature);

const recoveredSigner = ethers.verifyMessage(message, ethers.hexlify(signature));
console.log('\nRecovered signer:', recoveredSigner.toLowerCase());
console.log('Expected witness:', proof.witnesses[0].id.toLowerCase());
console.log('\nMatch:', recoveredSigner.toLowerCase() === proof.witnesses[0].id.toLowerCase() ? '✅ YES' : '❌ NO');

console.log('\n📝 How it works:');
console.log('1. Message format: identifier\\nowner\\ntimestamp\\nepoch');
console.log('2. ethers.verifyMessage() automatically:');
console.log('   - Converts message to bytes');
console.log('   - Adds "\\x19Ethereum Signed Message:\\n<length>" prefix');
console.log('   - Hashes with keccak256');
console.log('   - Recovers signer from signature');
console.log('');
console.log('This matches the SDK implementation:');
console.log('  createSignDataForClaim() -> ethers.verifyMessage()');

console.log('\n' + '='.repeat(70));

// Now check what the SDK's verifyProof does
console.log('\n🔍 HOW SDK VERIFIES:\n');
console.log('The SDK calls verifyProof() which:');
console.log('1. Gets witnesses from getWitnessesForClaim(epoch, identifier, timestamp)');
console.log('2. This calls the Reclaim contract on Optimism Sepolia');
console.log('3. Contract runs fetchWitnessesForClaim() deterministically');
console.log('4. Compares recovered signers with expected witnesses');
console.log('\nThe question: Does the Optimism Sepolia contract have the RIGHT witnesses?');
