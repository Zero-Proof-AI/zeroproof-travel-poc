/**
 * Local ZK Proof Verification Module
 * 
 * This module provides local verification of Groth16 ZK-SNARK proofs
 * extracted from Reclaim Protocol's attestor-core library.
 */

const { verifyProof: zkVerifyProof } = require('@reclaimprotocol/zk-symmetric-crypto');
const { 
    makeSnarkJsZKOperator,
    makeLocalFileFetch,
    makeRemoteFileFetch,
    CONFIG
} = require('@reclaimprotocol/zk-symmetric-crypto');
const { 
    concatenateUint8Arrays, 
    strToUint8Array, 
    generateIV 
} = require('@reclaimprotocol/tls');

const REDACTION_CHAR_CODE = 0;

// Cache for ZK operators to avoid re-initialization
const zkOperatorCache = {};

/**
 * Get or create a ZK operator for a specific algorithm
 */
function getZkOperator(algorithm, zkEngine = 'snarkjs') {
    const cacheKey = `${algorithm}-${zkEngine}`;
    
    if (!zkOperatorCache[cacheKey]) {
        console.log(`🔧 Initializing ZK operator: ${algorithm} (${zkEngine})`);
        
        // Determine environment (Node.js vs browser)
        const isNode = typeof process !== 'undefined' && process.versions && process.versions.node;
        const fetcher = isNode 
            ? makeLocalFileFetch()
            : makeRemoteFileFetch({ 
                baseUrl: 'https://witness.reclaimprotocol.org/zk-artifacts'
            });
        
        // Currently only snarkjs is supported
        if (zkEngine === 'snarkjs') {
            zkOperatorCache[cacheKey] = makeSnarkJsZKOperator({ 
                algorithm, 
                fetcher 
            });
        } else {
            throw new Error(`Unsupported ZK engine: ${zkEngine}`);
        }
    }
    
    return zkOperatorCache[cacheKey];
}

/**
 * Get the algorithm string from cipher suite
 */
function getZkAlgorithmForCipherSuite(cipherSuite) {
    // Map cipher suites to ZK algorithms
    // This is a simplified mapping - extend as needed
    if (cipherSuite.includes('AES_128_GCM') || cipherSuite.includes('AES_256_GCM')) {
        return 'aes-128-gcm';
    } else if (cipherSuite.includes('CHACHA20')) {
        return 'chacha20';
    }
    // Default to chacha20
    return 'chacha20';
}

/**
 * Get pure ciphertext (without headers/tags for some cipher suites)
 */
function getPureCiphertext(ciphertext, cipherSuite) {
    // For GCM modes, remove the 16-byte authentication tag
    if (cipherSuite.includes('GCM')) {
        return ciphertext.slice(0, -16);
    }
    return ciphertext;
}

/**
 * Get record IV from ciphertext based on cipher suite
 */
function getRecordIV(ciphertext, cipherSuite) {
    // For TLS 1.2, record IV is usually empty (uses fixed IV)
    // For TLS 1.3, each record has explicit nonce
    // Simplified - may need adjustment based on actual implementation
    return new Uint8Array(0);
}

/**
 * Check if plaintext is fully redacted
 */
function isFullyRedacted(plaintext) {
    return plaintext.every(byte => byte === REDACTION_CHAR_CODE);
}

/**
 * Check if redaction is congruent (redacted parts match)
 */
function isRedactionCongruent(plaintext1, plaintext2) {
    if (plaintext1.length !== plaintext2.length) {
        return false;
    }
    
    for (let i = 0; i < plaintext1.length; i++) {
        // If redacted in first, must be redacted in second
        if (plaintext1[i] === REDACTION_CHAR_CODE && plaintext2[i] !== REDACTION_CHAR_CODE) {
            return false;
        }
        // If not redacted in first, must match in second
        if (plaintext1[i] !== REDACTION_CHAR_CODE && plaintext1[i] !== plaintext2[i]) {
            return false;
        }
    }
    
    return true;
}

/**
 * Verify a single ZK proof packet
 * 
 * This is the core verification function extracted from attestor-core
 */
async function verifyProofPacket({
    proof,
    ciphertext,
    iv,
    recordNumber = 0,
    cipherSuite,
    logger = console
}) {
    const {
        proofData,
        proofJson,
        decryptedRedactedCiphertext,
        redactedPlaintext,
        startIdx,
        toprf
    } = proof;

    const algorithm = getZkAlgorithmForCipherSuite(cipherSuite);
    
    // Get the ciphertext chunk we received from the server
    // The ZK library will verify that the decrypted redacted
    // ciphertext matches the ciphertext received from the server
    const ciphertextChunk = ciphertext.slice(
        startIdx, 
        startIdx + redactedPlaintext.length
    );

    // Redact ciphertext if plaintext is redacted
    // to prepare for decryption in ZK circuit
    for (let i = 0; i < ciphertextChunk.length; i++) {
        if (redactedPlaintext[i] === REDACTION_CHAR_CODE) {
            ciphertextChunk[i] = REDACTION_CHAR_CODE;
        }
    }

    // Verify redaction congruence
    if (!isRedactionCongruent(redactedPlaintext, decryptedRedactedCiphertext)) {
        throw new Error('Redacted ciphertext not congruent');
    }

    // Generate nonce for this record
    const recordIV = getRecordIV(ciphertext, cipherSuite);
    let nonce = concatenateUint8Arrays([iv, recordIV]);
    if (!recordIV.length) {
        nonce = generateIV(nonce, recordNumber);
    }

    // Get ZK operator
    const operator = getZkOperator(algorithm);

    // Verify the actual Groth16 ZK-SNARK proof
    logger.debug?.(`🔍 Verifying Groth16 ZK proof at offset ${startIdx}...`);
    
    await zkVerifyProof({
        proof: {
            algorithm,
            proofData: proofData.length 
                ? proofData 
                : strToUint8Array(proofJson),
            plaintext: decryptedRedactedCiphertext,
        },
        publicInput: {
            ciphertext: ciphertextChunk,
            iv: nonce,
            offsetBytes: startIdx
        },
        operator,
        logger
    });

    logger.debug?.(`✅ Groth16 ZK proof verified at offset ${startIdx}`);
}

/**
 * Verify complete ZK packet with all proofs
 * 
 * Main verification function matching attestor-core's verifyZkPacket()
 */
async function verifyZkPacket({
    cipherSuite,
    ciphertext,
    zkReveal,
    iv,
    recordNumber = 0,
    zkEngine = 'snarkjs',
    logger = console
}) {
    if (!zkReveal) {
        throw new Error('No ZK reveal provided');
    }

    const { proofs } = zkReveal;
    
    if (!proofs || proofs.length === 0) {
        throw new Error('No proofs in ZK reveal');
    }

    logger.info?.(`🔍 Verifying ${proofs.length} ZK proof(s)...`);

    // Get pure ciphertext (remove authentication tags etc.)
    const pureCiphertext = getPureCiphertext(ciphertext, cipherSuite);

    // Verify all proofs in parallel
    await Promise.all(
        proofs.map(async (proof, i) => {
            try {
                await verifyProofPacket({
                    proof,
                    ciphertext: pureCiphertext,
                    iv,
                    recordNumber,
                    cipherSuite,
                    logger
                });
            } catch (e) {
                e.message += ` (chunk ${i}, startIdx ${proof.startIdx})`;
                throw e;
            }
        })
    );

    logger.info?.(`✅ All ${proofs.length} ZK proof(s) verified successfully`);

    // Reconstruct redacted plaintext from all proof chunks
    const redactedPlaintext = new Uint8Array(pureCiphertext.length).fill(REDACTION_CHAR_CODE);
    for (const proof of proofs) {
        redactedPlaintext.set(proof.redactedPlaintext, proof.startIdx);
    }

    return { 
        redactedPlaintext,
        verified: true 
    };
}

/**
 * Verify a complete Reclaim proof (wrapper for convenience)
 */
async function verifyReclaimProof(proof, options = {}) {
    const { logger = console } = options;

    try {
        // Extract ZK-related data from proof
        // Note: This assumes the proof structure from zkFetch
        const context = JSON.parse(proof.claimData.context);
        
        if (!context.zkReveal) {
            logger.warn?.('⚠️  No ZK reveal in proof - signature-only verification');
            return { verified: false, reason: 'no-zk-data' };
        }

        // Verify the ZK packet
        const result = await verifyZkPacket({
            cipherSuite: context.extractedParameters.cipherSuite || 'TLS_CHACHA20_POLY1305_SHA256',
            ciphertext: Buffer.from(context.extractedParameters.ciphertext || '', 'hex'),
            zkReveal: context.zkReveal,
            iv: Buffer.from(context.extractedParameters.iv || '', 'hex'),
            recordNumber: context.extractedParameters.recordNumber || 0,
            logger
        });

        return result;
    } catch (error) {
        logger.error?.('❌ ZK verification failed:', error.message);
        throw error;
    }
}

module.exports = {
    verifyZkPacket,
    verifyProofPacket,
    verifyReclaimProof,
    getZkOperator,
    // Utility functions
    getZkAlgorithmForCipherSuite,
    getPureCiphertext,
    isRedactionCongruent,
    REDACTION_CHAR_CODE
};
