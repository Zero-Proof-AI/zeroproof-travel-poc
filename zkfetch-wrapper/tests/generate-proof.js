const { ReclaimClient } = require('@reclaimprotocol/zk-fetch');
const fs = require('fs');
require('dotenv').config();

async function generateProof() {
    console.log('🔄 Generating fresh proof...\n');

    const client = new ReclaimClient(
        process.env.RECLAIM_APP_ID,
        process.env.RECLAIM_APP_SECRET
    );

    const publicOptions = {
        method: 'GET',
        headers: {
            'accept': 'application/json'
        }
    };

    const privateOptions = {
        responseMatches: [{
            type: 'regex',
            value: '"origin":\\s*"(?<origin>[^"]+)"'
        }]
    };

    const proof = await client.zkFetch(
        'https://httpbin.org/get',
        publicOptions,
        privateOptions
    );

    // Save proof to file
    fs.writeFileSync('proof-structure.json', JSON.stringify(proof, null, 2));
    console.log('✅ Proof generated and saved to proof-structure.json\n');

    return proof;
}

// Only run if called directly (not when required)
if (require.main === module) {
    generateProof()
        .then(() => process.exit(0))
        .catch(error => {
            console.error('❌ Error generating proof:', error.message);
            process.exit(1);
        });
}

module.exports = { generateProof };
