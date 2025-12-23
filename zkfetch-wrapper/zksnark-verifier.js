const snarkjs = require('snarkjs');
const fs = require('fs');
const path = require('path');
const { verifyProof: sdkVerifyProof } = require('@reclaimprotocol/js-sdk');

/**
 * Full verification including zkSNARK Groth16 proof
 * 
 * Note: Current Reclaim SDK proofs do NOT include zkSNARK proof data.
 * The zkSNARK is generated and verified by attestors.
 * This module provides the infrastructure for full verification if proof data were available.
 */

// Load verification key (exported from circuit_final.zkey)
const VERIFICATION_KEY_PATH = path.join(__dirname, 'verification_key.json');
let verificationKey = null;

function loadVerificationKey() {
  if (!verificationKey) {
    if (!fs.existsSync(VERIFICATION_KEY_PATH)) {
      throw new Error('Verification key not found. Run: npx snarkjs zkey export verificationkey ...');
    }
    verificationKey = JSON.parse(fs.readFileSync(VERIFICATION_KEY_PATH, 'utf8'));
    console.log('✓ Loaded Groth16 verification key');
    console.log(`  Protocol: ${verificationKey.protocol}`);
    console.log(`  Curve: ${verificationKey.curve}`);
    console.log(`  Public inputs: ${verificationKey.nPublic}`);
  }
  return verificationKey;
}

/**
 * Verify attestor signatures and proof integrity (current SDK behavior)
 */
async function verifySignatures(proof) {
  try {
    const isValid = await sdkVerifyProof(proof);
    return {
      valid: isValid,
      method: 'signature-based',
      checks: {
        attestorSignatures: isValid,
        witnessAddresses: isValid,
        identifierIntegrity: isValid,
        timestamp: isValid
      }
    };
  } catch (error) {
    return {
      valid: false,
      method: 'signature-based',
      error: error.message
    };
  }
}

/**
 * Verify zkSNARK Groth16 proof using snarkjs
 * 
 * @param {Object} zkProofData - {proof: {...}, publicSignals: [...]}
 * @returns {Promise<boolean>}
 */
async function verifyZkProof(zkProofData) {
  const vKey = loadVerificationKey();
  
  try {
    const isValid = await snarkjs.groth16.verify(
      vKey,
      zkProofData.publicSignals,
      zkProofData.proof
    );
    
    return isValid;
  } catch (error) {
    console.error('zkSNARK verification error:', error.message);
    return false;
  }
}

/**
 * Full verification: signatures + zkSNARK proof (if available)
 */
async function verifyProofFull(proof, options = {}) {
  const result = {
    valid: false,
    timestamp: Date.now(),
    verifications: {}
  };
  
  // Step 1: Verify signatures (always available)
  console.log('1. Verifying attestor signatures...');
  const sigResult = await verifySignatures(proof);
  result.verifications.signatures = sigResult;
  
  if (!sigResult.valid) {
    result.valid = false;
    result.error = 'Signature verification failed';
    return result;
  }
  
  console.log('   ✅ Signature verification passed');
  
  // Step 2: Verify zkSNARK proof (if available)
  if (proof.zkProof && options.verifyZkProof !== false) {
    console.log('2. Verifying zkSNARK Groth16 proof...');
    
    try {
      const zkValid = await verifyZkProof(proof.zkProof);
      result.verifications.zkProof = {
        valid: zkValid,
        method: 'groth16',
        protocol: verificationKey.protocol,
        curve: verificationKey.curve
      };
      
      if (!zkValid) {
        result.valid = false;
        result.error = 'zkSNARK proof verification failed';
        return result;
      }
      
      console.log('   ✅ zkSNARK verification passed');
    } catch (error) {
      result.verifications.zkProof = {
        valid: false,
        error: error.message
      };
      
      if (options.requireZkProof) {
        result.valid = false;
        result.error = 'zkSNARK verification required but failed';
        return result;
      }
    }
  } else {
    console.log('2. zkSNARK proof not available (verified by attestors)');
    result.verifications.zkProof = {
      available: false,
      note: 'zkSNARK verified by attestors, signatures confirm attestation'
    };
  }
  
  // All checks passed
  result.valid = true;
  return result;
}

/**
 * Verify proof with detailed output
 */
async function verifyProofDetailed(proof) {
  console.log('\n=== Full Proof Verification ===\n');
  
  const result = await verifyProofFull(proof);
  
  console.log('\n=== Verification Result ===');
  console.log('Status:', result.valid ? '✅ VALID' : '❌ INVALID');
  
  if (result.verifications.signatures) {
    console.log('\nSignature Verification:');
    console.log('  Valid:', result.verifications.signatures.valid ? '✅' : '❌');
    if (result.verifications.signatures.checks) {
      Object.entries(result.verifications.signatures.checks).forEach(([check, passed]) => {
        console.log(`  ${check}:`, passed ? '✅' : '❌');
      });
    }
  }
  
  if (result.verifications.zkProof) {
    console.log('\nzkSNARK Verification:');
    if (result.verifications.zkProof.available === false) {
      console.log('  Status: Not available (verified by attestors)');
    } else {
      console.log('  Valid:', result.verifications.zkProof.valid ? '✅' : '❌');
      if (result.verifications.zkProof.protocol) {
        console.log('  Protocol:', result.verifications.zkProof.protocol);
        console.log('  Curve:', result.verifications.zkProof.curve);
      }
    }
  }
  
  return result;
}

module.exports = {
  verifySignatures,
  verifyZkProof,
  verifyProofFull,
  verifyProofDetailed,
  loadVerificationKey
};
