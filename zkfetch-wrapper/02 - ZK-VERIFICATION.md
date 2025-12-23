# Cryptographic Verification in Reclaim Protocol

## Architecture Overview

**Two-Layer Verification System:**

Reclaim Protocol uses **both** zkSNARKs (Groth16) **and** ECDSA signatures for comprehensive verification:

### Layer 1: ZK-SNARK Proof (Groth16) - Selective Disclosure

**Generated**: Client-side using `@reclaimprotocol/zk-symmetric-crypto`
- **Algorithm**: Groth16 zkSNARK
- **Circuits**: Circom circuits (ChaCha20, AES-256-CTR, AES-128-CTR)
- **Backend**: snarkjs or gnark
- **Purpose**: Prove knowledge of TLS encryption keys without revealing them
- **Privacy**: Enables selective disclosure (reveal "status: APPROVED", hide credit card)
- **Location**: Generated locally on client device
- **Verification**: 
  - Off-chain: SDK verifies ZK proof validity locally
  - On-chain: NOT verified directly (bound to signed transcript hash, implicitly trusted for gas efficiency)

### Layer 2: Attestor Signatures (ECDSA)

**Signed**: By decentralized attestor network
- **Algorithm**: ECDSA (same as Ethereum transactions)
- **Purpose**: Attest that ZK proof was verified AND encrypted TLS traffic was observed
- **Pre-signing**: Attestor verifies ZK proof using attestor-core before signing
- **Trust Model**: Attestor only signs if ZK proof is valid
- **Verification**: ECDSA signatures verified locally (JS SDK) or on-chain (smart contracts)
- **Implementation**: Uses `ethers.verifyMessage()` or Solidity `ECRECOVER`
- **What's signed**: Transcript hash (which includes the embedded ZK proof)
- **Location**: ECDSA verified everywhere; ZK proof verified only by attestor

## How It Works

### Trust Model
1. **Client connects through attestor proxy**: TLS connection is proxied through attestor
2. **Attestor observes encrypted traffic**: Cannot see plaintext due to TLS encryption
3. **Client creates claim with selective disclosure**: Redacts sensitive fields locally
4. **Attestor signs the claim**: ECDSA signature attesting to the encrypted traffic
5. **Anyone verifies signatures**: Check that trusted attestors signed the claim

### Benefits
- ✅ **Gas efficient**: Signature verification is cheap (~3k gas per signature)
- ✅ **Fast**: ECDSA verification is instant
- ✅ **Privacy preserved**: TLS encryption + selective disclosure hide sensitive data
- ✅ **Decentralized**: Multiple independent attestors (honest-majority assumption)

## Complete Verification Flow

```
┌─────────────────────────────────────────────────────────────┐
│ CLIENT SIDE                                                 │
├─────────────────────────────────────────────────────────────┤
│ 1. Connect to target API through attestor proxy            │
│ 2. Send HTTPS request (TLS encrypted)                       │
│ 3. Receive HTTPS response (TLS encrypted)                   │
│ 4. Decrypt response locally (have TLS session keys)         │
│ 5. Generate ZK proof locally:                               │
│    - Use @reclaimprotocol/zk-symmetric-crypto              │
│    - Circom circuits + snarkjs (Groth16)                    │
│    - Prove knowledge of encryption keys                     │
│    - Enable selective disclosure (redact fields)            │
│ 6. Create claim with embedded ZK proof                      │
│ 7. Send claim to attestor for signing                       │
└─────────────────────────────────────────────────────────────┘
                         ↓
┌─────────────────────────────────────────────────────────────┐
│ ATTESTOR SIDE (Proxy Server + attestor-core)                │
├─────────────────────────────────────────────────────────────┤
│ 8. Has observed encrypted TLS traffic (proxied connection)  │
│ 9. Receives claim with embedded ZK proof from client        │
│10. ✅ VERIFIES ZK PROOF using attestor-core (Groth16)       │
│11. Validates claim matches observed encrypted traffic       │
│12. Signs transcript hash with ECDSA (ONLY if ZK valid)      │
│13. Returns signed claim to client                           │
└─────────────────────────────────────────────────────────────┘
                         ↓
┌─────────────────────────────────────────────────────────────┐
│ VERIFIER - OFF-CHAIN (@reclaimprotocol/js-sdk)              │
├─────────────────────────────────────────────────────────────┤
│14. Verify attestor ECDSA signatures (ethers.verifyMessage)  │
│15. Trust that attestor verified ZK proof (before signing)   │
│16. Redacted fields remain hidden (zero-knowledge property)  │
│    Note: js-sdk does NOT re-verify ZK proof                 │
└─────────────────────────────────────────────────────────────┘
                         ↓
┌─────────────────────────────────────────────────────────────┐
│ VERIFIER - ON-CHAIN (Reclaim.sol contract)                  │
├─────────────────────────────────────────────────────────────┤
│16. Verify ONLY attestor ECDSA signatures (ECRECOVER)        │
│17. ZK proof NOT verified on-chain (gas efficiency)          │
│18. ZK proof bound to signed transcript hash (trusted)       │
│19. Transaction reverts if signatures invalid                │
└─────────────────────────────────────────────────────────────┘
```

## Code Locations

### ZK-SNARK Verification (Groth16)
**File**: `@reclaimprotocol/attestor-core/lib/utils/zk.js`

```javascript
// Main verification function
async function verifyZkPacket({ cipherSuite, ciphertext, zkReveal, ... }) {
    // Line 227: Verify each proof chunk
    await verifyProofPacket(proof);
    
    // Line 277: Actual Groth16 verification
    await zk_symmetric_crypto_1.verifyProof({
        proof: { algorithm, proofData, plaintext },
        publicInput: { ciphertext, iv, offsetBytes },
        operator  // snarkjs or gnark operator
    });
}
```

**File**: `@reclaimprotocol/zk-symmetric-crypto/lib/zk.js`

```javascript
// Line 40-53: Groth16 verification
async function verifyProof(opts) {
    const publicSignals = getPublicSignals(opts);
    const { proof: { proofData }, operator, logger } = opts;
    
    // THIS IS THE ACTUAL GROTH16 VERIFICATION
    verified = await operator.groth16Verify(
        publicSignals, 
        proofData, 
        logger
    );
    
    if (!verified) {
        throw new Error('invalid proof');
    }
}
```

### Signature Verification (ECDSA)
**File**: `@reclaimprotocol/js-sdk/dist/index.js`

```javascript
// Line 1783: Main verification function
function verifyProof(proofOrProofs, allowAiWitness) {
    // 1. Get witnesses for this claim
    const witnesses = getWitnessesForClaim(claim);
    
    // 2. Verify identifier matches claim info
    const expectedId = getIdentifierFromClaimInfo(claimInfo);
    
    // 3. Verify all witness signatures
    assertValidSignedClaim(signedClaim, witnesses);
}

// Lines 1308-1314: Signature recovery
function recoverSignersOfSignedClaim(claim) {
    return claim.signatures.map(signature => {
        return ethers.verifyMessage(serializedClaim, signature);
    });
}
```

## Local ZK Verification

The `zk-verify.js` module in this wrapper provides the ability to:
1. Extract ZK proof data from Reclaim proofs (if available)
2. Verify Groth16 ZK-SNARKs locally using attestor-core logic
3. Replicate server-side verification for debugging/auditing

**NOTE**: Current zkFetch SDK does NOT return ZK proof data in the response. The ZK verification happens server-side only, and clients receive only the signed claim.

To enable local ZK verification, you would need:
- Modified SDK that includes `zkReveal` data in responses
- Or direct access to attestor's ZK proof generation flow

## On-Chain Verification

### Current Implementation (Reclaim.sol)
- ✅ Verifies witness ECDSA signatures
- ✅ Checks witness selection based on epoch
- ✅ Validates claim identifier calculation
- ❌ Does NOT verify Groth16 ZK proofs (by design)

### Why No On-Chain ZK Verification?
1. **Gas cost**: Groth16 verification on-chain costs ~280k gas minimum
2. **Trust model**: Attestors are trusted after signature verification
3. **Multisig security**: Multiple independent attestors must sign
4. **Efficiency**: Signature verification is 100x cheaper

### If You Need On-Chain ZK Verification
You can implement Groth16 verification on-chain using:
- Solidity libraries: `snarkjs` verification contracts
- Pre-compiles: Use EVM bn256 pairing operations
- L2 solutions: Verify on cheaper chains

**Trade-off**: Pay high gas costs for trustless verification vs. trust multiple attestors with cheap signature checks.

## Security Considerations

### What Attestor Signatures Prove
✅ Attestor observed the encrypted TLS traffic  
✅ Attestor verified the ZK proof client submitted  
✅ Claim data matches what was proven in ZK  
✅ Timestamp and epoch are correct  

### What They Don't Prove
❌ The TLS connection was to the real server (trust DNS/certificates)  
❌ Server's API response was legitimate (trust the API)  
❌ No attestor collusion (trust attestor selection process)  

### Trust Assumptions
- **Attestor honesty**: Assume attestors correctly verify ZK proofs
- **Attestor availability**: At least N of M attestors must be online
- **Attestor independence**: Attestors don't collude
- **ZK soundness**: Groth16 circuit correctly enforces TLS decryption

## Testing ZK Verification

```bash
# Test signature verification (always available)
node test-httpbin-origin.js

# Test local ZK verification (requires ZK proof data)
node test-zk-verify.js
```

## References

- **Groth16 Paper**: https://eprint.iacr.org/2016/260.pdf
- **snarkjs**: https://github.com/iden3/snarkjs
- **Reclaim Protocol**: https://www.reclaimprotocol.org/
- **ZK Circuits**: ChaCha20-CTR and AES-GCM decryption proofs
