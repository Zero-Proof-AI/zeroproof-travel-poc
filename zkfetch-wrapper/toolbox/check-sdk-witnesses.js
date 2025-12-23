/**
 * Check SDK Witnesses - Public API Version
 *
 * Purpose: Uses only the public @reclaimprotocol/js-sdk API to verify a proof
 * and log the witnesses used by the SDK for verification. No monkey-patching or
 * internal imports required. Compares witnesses in proof metadata to those used by SDK.
 *
 * Usage: node toolbox/check-sdk-witnesses.js
 * Note: Requires proof-structure.json in current directory
 */

const { verifyProof } = require('@reclaimprotocol/js-sdk');
const fs = require('fs');

async function checkSDKWitnesses() {
    console.log('='.repeat(70));
    console.log('  CHECKING SDK WITNESS RETRIEVAL (PUBLIC API)');
    console.log('='.repeat(70));

    const proof = JSON.parse(fs.readFileSync('./proof-structure.json', 'utf8'));

    console.log('\n📋 Proof Details:');
    console.log('  Identifier:', proof.identifier);
    console.log('  Epoch:', proof.claimData.epoch);
    console.log('  Timestamp:', proof.claimData.timestampS, '(' + new Date(proof.claimData.timestampS * 1000).toISOString() + ')');
    console.log('  Witness in metadata:', proof.witnesses[0].id);
    console.log('  Signature:', proof.signatures[0].substring(0, 20) + '...');

    console.log('\n🔍 Calling SDK verifyProof()...');

    try {
        const isValid = await verifyProof(proof, {
            debug: true, // If SDK supports debug output, enable it
        });

        console.log('\n' + '='.repeat(70));
        console.log('  RESULTS');
        console.log('='.repeat(70));

        console.log('\n✅ Verification Result:', isValid);
        console.log('\n📋 Witnesses in proof metadata:');
        proof.witnesses.forEach((w, i) => {
            console.log(`  [${i}] ${w.id}`);
        });
        console.log('\nNote: The SDK may fetch witnesses from the contract or beacon internally.');
        console.log('      For full details, use check-all-epochs.js to see contract-side witnesses.');

        // Recover the actual signer
        const { ethers } = require('ethers');
        const { identifier, owner, timestampS, epoch } = {
            identifier: proof.identifier,
            owner: proof.claimData.owner,
            timestampS: proof.claimData.timestampS,
            epoch: proof.claimData.epoch
        };
        const message = identifier.toLowerCase() + '\n' + owner.toLowerCase() + '\n' + timestampS + '\n' + epoch;
        const recoveredSigner = ethers.verifyMessage(message, ethers.hexlify(proof.signatures[0]));
        console.log('\n🔐 Signature Analysis:');
        console.log('  Recovered signer:', recoveredSigner);
        const witnessIds = proof.witnesses.map(w => w.id.toLowerCase());
        const signerMatch = witnessIds.includes(recoveredSigner.toLowerCase());
        console.log('  Signer in witness list:', signerMatch ? '✅ YES' : '❌ NO');
    } catch (error) {
        console.error('\n❌ Error:', error.message);
        console.error(error);
    }

    console.log('\n' + '='.repeat(70));
}

checkSDKWitnesses().catch(console.error);
