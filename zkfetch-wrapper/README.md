# zkFetch Wrapper - Privacy-Preserving Verifiable API Proofs

Generate cryptographically verifiable proofs of API responses with **selective disclosure** using Reclaim Protocol's zkTLS technology.

## Overview

zkFetch Wrapper is a production-ready service that generates cryptographically verifiable proofs of API responses while protecting sensitive data. Perfect for privacy-preserving applications requiring trustless data verification.

### Key Capabilities

✅ **Verifiable API Proofs** - Cryptographically prove API responses  
✅ **Zero-Knowledge Proofs** - Groth16 zkSNARKs for selective disclosure  
✅ **Selective Disclosure** - Hide credit cards, SSNs, emails while proving claims  
✅ **Attestor Signatures** - ECDSA signatures from decentralized witnesses  
✅ **Local ZK Generation** - ZK proofs generated client-side (Circom + snarkjs)  
✅ **Hybrid Verification** - Off-chain: ECDSA + ZK proof | On-chain: ECDSA only  
✅ **Privacy-Preserving** - Redact fields before sharing proofs  
✅ **On-chain Compatible** - Smart contract verification ready  
✅ **Decentralized Attestors** - Trustworthy witness network  

### How It Works

```
┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│   zkFetch    │────▶│   Attestor   │────▶│  Verifier    │
│   Client     │     │   Proxy      │     │  (Anyone)    │
└──────────────┘     └──────────────┘     └──────────────┘
  Proxies request      Forwards to API      Verifies
  via attestor         Verifies ZKP         signatures on
  Generates ZKP        Signs attestation    claim data
  with selective       on claim data         
  disclosure                                (cheap!)
```

**Flow:**
1. Client sends request through attestor proxy
2. Attestor forwards to target API, observes encrypted TLS traffic
3. Client generates ZK proof locally (Circom + Groth16) for selective disclosure
4. Client creates claim with ZK proof embedded, sends to attestor
5. **Attestor verifies ZK proof using attestor-core (Groth16 verification)**
6. Attestor signs claim with ECDSA **only if ZK proof is valid**
7. Off-chain verification (js-sdk): Verifies ECDSA signatures (trusts attestor's ZK verification)
8. On-chain verification: Verifies ECDSA signatures only (trusts attestor, gas efficient)

**See [ZK-VERIFICATION.md](./ZK-VERIFICATION.md)

---

## Quick Start

### 1. Installation

```bash
npm install
```

### 2. Get Reclaim Credentials

1. Visit https://dev.reclaimprotocol.org/
2. Create an application
3. Add an "Https" provider
4. Copy APP_ID and APP_SECRET

### 3. Configure Environment

Create `.env`:

```bash
RECLAIM_APP_ID=0xYourAppIdHere
RECLAIM_APP_SECRET=0xYourAppSecretHere
PORT=8003
```

### 4. Start Server

```bash
npm start
```

Server runs on `http://localhost:8003`

---

## API Endpoints

### `POST /zkfetch` - Generate Proof

Generate a zkTLS proof with optional selective disclosure.

**Request:**
```json
{
  "url": "https://api.example.com/data",
  "publicOptions": {
    "method": "GET",
    "headers": { "Accept": "application/json" }
  },
  "redactions": [
    { "jsonPath": "$.creditCard", "replacement": "REDACTED" }
  ]
}
```

**Response:**
```json
{
  "success": true,
  "data": { ... },
  "proof": { ... },
  "metadata": {
    "timestamp": 1734518400000,
    "onchain_compatible": true
  }
}
```

### `POST /verify` - Verify Proof

Verify a proof's cryptographic validity.

**Request:**
```json
{
  "proof": { ... }
}
```

**Response:**
```json
{
  "success": true,
  "valid": true,
  "extractedData": { ... }
}
```

### `GET /health` - Health Check

Check service status.

---

## Selective Disclosure

### Hide Sensitive Data While Proving Claims

**Example: Prove booking without revealing payment info**

```javascript
const response = await fetch('http://localhost:8003/zkfetch', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({
    url: 'https://api.airline.com/booking/AA12345',
    publicOptions: { method: 'GET' },
    redactions: [
      { jsonPath: '$.payment.creditCard', replacement: '****-****-****-XXXX' },
      { jsonPath: '$.payment.cvv', replacement: '***' }
    ]
  })
});

// Proof shows booking details but hides credit card
```

### Common Redaction Patterns

```javascript
// Credit card
{ jsonPath: '$.creditCard', replacement: '****-****-****-XXXX' }

// Email
{ jsonPath: '$.email', replacement: 'user@REDACTED.com' }

// SSN
{ jsonPath: '$.ssn', replacement: 'XXX-XX-XXXX' }

// Account number
{ jsonPath: '$.account.number', replacement: 'ACCT_REDACTED' }

// Balance threshold
{ jsonPath: '$.balance', replacement: 'ABOVE_$1000' }
```

---

## Use Cases

### 1. Financial Privacy
**Prove**: Balance >= $1000  
**Hide**: Exact balance, account number, SSN  
**Use**: Loan applications, credit checks

### 2. Identity Privacy
**Prove**: Age >= 18, KYC verified  
**Hide**: Full name, DOB, address, ID number  
**Use**: Age-restricted services, compliance

### 3. Employment Privacy
**Prove**: Active employee at Company X  
**Hide**: Salary, SSN, bank account  
**Use**: Background checks, verification

### 4. Transaction Privacy
**Prove**: Payment made for booking AA12345  
**Hide**: Credit card, CVV, billing address  
**Use**: Booking confirmations, expense reimbursement

---

## Architecture

### How It Works

```
1. YOU (Prover)
   ├─ Make API request through attestor proxy
   ├─ Receive response and decrypt locally
   ├─ Create claim with selective disclosure (choose what to reveal)
   └─ Request attestor signature on the claim

2. ATTESTORS (Reclaim Network)
   ├─ Proxy HTTPS traffic to target API
   ├─ Observe encrypted TLS traffic
   ├─ Verify domain & certificate
   ├─ Sign the claim with ECDSA signature
   └─ Return signed attestation

3. VERIFIERS (Anyone)
   ├─ Receive proof with redacted fields
   ├─ Verify cryptographic signatures
   ├─ See only non-redacted data
   └─ Cannot access hidden fields
```

### Privacy Guarantees

**You Control:**
- ✅ Full plaintext response (local decryption)
- ✅ Which fields to reveal/redact
- ✅ Replacement values for redacted fields
- ✅ Who sees the proof

**Verifiers See:**
- ✅ Non-redacted fields (your choice)
- ✅ Cryptographic proof validity
- ✅ Attestor signatures
- ❌ Redacted fields (replaced with placeholders)

**Attestors See:**
- ✅ Encrypted traffic to target domain
- ✅ TLS certificate validity
- ❌ Plaintext response (TLS encrypted)
- ❌ Redacted fields (client-side selective disclosure)

---

## Testing

### Run Demo

```bash
node toolbox/demo-selective-disclosure.js
```

Shows:
- What prover sees (full data)
- What verifier sees (redacted proof)
- Verification process

### Run Tests

```bash
node tests/test-selective-disclosure.js
```

Tests:
- Basic field redaction
- Multi-field redaction
- Age threshold proofs
- Verification


---

## Documentation

- **[ZKTLS_ARCHITECTURE.md](./ZKTLS_ARCHITECTURE.md)** - zkTLS architecture details
```
┌──────────────────────────────────────────────────────┐
│ CURRENT FLOW (Signature-Only)                        │
├──────────────────────────────────────────────────────┤
│                                                      │
│  Client  ──[ZK Proof]──▶  Attestor                   │
│                             │                        │
│                             ├─ Verify ZK ✓           │
│                             └─ Sign claim            │
│                                  │                   │
│  Client  ◀──[Signatures]─────────┘                   │
│     │                                                │
│     └─ Verify signatures ✓                           │
│                                                      │
└──────────────────────────────────────────────────────┘

┌──────────────────────────────────────────────────────┐
│ FUTURE FLOW (Full ZK Verification)                   │
├──────────────────────────────────────────────────────┤
│                                                      │
│  Client  ──[ZK Proof]──▶  Attestor                   │
│                             │                        │
│                             ├─ Verify ZK ✓           │
│                             └─ Sign claim            │
│                                  │                   │
│  Client  ◀──[Signatures + ZK]────┘                   │
│     │                                                │
│     ├─ Verify signatures ✓                           │
│     └─ Verify ZK proof ✓ (using zk-verify.js)        │
│                                                      │
└──────────────────────────────────────────────────────┘
```

---

## Advanced Features

### Cryptographic Verification

Reclaim proofs use **ECDSA signatures** from attestors, not zkSNARKs:

```javascript
const { verifyProof } = require('@reclaimprotocol/js-sdk');

const isValid = await verifyProof(proof);
console.log('Attestation valid:', isValid);
// Verifies: ECDSA signatures from decentralized witnesses
```

### Response Matching (Threshold Proofs)

Prove data matches pattern without revealing exact value:

```javascript
{
  privateOptions: {
    responseMatches: [
      { type: 'regex', value: '"balance":\\s*[1-9]\\d{3,}' } // >= 1000
    ]
  },
  redactions: [
    { jsonPath: '$.balance', replacement: '>= $1000' }
  ]
}
```

---

## Security Considerations

### What Gets Proven

✅ Data came from the specified API domain  
✅ Encrypted traffic witnessed by decentralized attestors  
✅ You can decrypt the response  
✅ Response matches claimed values  
✅ Timestamp is authentic

### What Stays Private

🔒 Redacted fields (hidden from everyone)  
🔒 Private headers (Authorization tokens)  
🔒 Full response body (only revealed parts shown)  
🔒 Session keys (you hold them)

### Trust Model

**You trust:**
- Honest-majority attestor network (decentralized, staked, slashable)
- TLS/HTTPS infrastructure (same as normal web)
- Certificate Authorities (same as all HTTPS)

**Attestors CANNOT:**
- Decrypt your TLS traffic (don't have session keys)
- See redacted fields (client-side selective disclosure)
- Forge attestations (don't have your private keys)

---

## Troubleshooting

### "Application not found" Error

- Verify APP_ID and APP_SECRET in `.env`
- Check application status at https://dev.reclaimprotocol.org/
- Ensure "Https" provider is added and activated

### Verification Failing

- Verify the SAME proof you generated (don't generate new proof for each verification)
- Use proper HTTP client (axios), not shell/curl (JSON handling issues)
- Check proof structure in `proof-structure.json`

---

## Resources

- **Reclaim Protocol**: https://reclaimprotocol.org/
- **Developer Portal**: https://dev.reclaimprotocol.org/
- **Documentation**: https://docs.reclaimprotocol.org/
- **Whitepaper**: https://link.reclaimprotocol.org/whitepaper-draft-v2

---

## Summary

**zkFetch Wrapper provides production-ready zkTLS proofs with selective disclosure:**

1. 🔐 **You decrypt locally** - Full control over plaintext
2. 🎭 **You choose what to reveal** - Selective disclosure via JSON Path
3. ✅ **Verifiers see only what you allow** - Privacy preserved
4. 🛡️ **Cryptographically verifiable** - Cannot be forged

**Transform data sharing: From "trust me" to "verify it yourself"** 🚀
