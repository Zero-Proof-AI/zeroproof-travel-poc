# ZeroProof Smart Contracts

Multi-proof verification system supporting SP1 zkVM and Reclaim zkTLS proofs.

## Quick Links

- 📖 [Deployment Guide](DEPLOY.md) - Complete deployment instructions
- 🏗️ [Deployed Contracts](#deployed-contracts)
- 🧪 [Testing](#testing)
- 📚 [Architecture](#architecture)

---

## Deployed Contracts

### Sepolia Testnet

| Contract | Address | Purpose |
|----------|---------|---------|
| **ZeroProof** | [`0x9C33252D29B41Fe2706704a8Ca99E8731B58af41`](https://sepolia.etherscan.io/address/0x9c33252d29b41fe2706704a8ca99e8731b58af41) | Entry point for all proofs |
| **SP1 Verifier** | `0x53A9038dCB210D210A7C973fA066Fd2C50aa8847` | zkVM proof verification |
| **Reclaim Verifier** | `0xAe94FB09711e1c6B057853a515483792d8e474d0` | zkTLS proof verification |

---

## Quick Deploy

```bash
# 1. Setup environment
cp .env.example .env
# Edit .env with your keys

# 2. Install dependencies
forge install

# 3. Build
forge build

# 4. Deploy
forge script script/DeployZeroProof.s.sol:DeployZeroProof \
  --rpc-url $RPC_URL \
  --private-key $PRIVATE_KEY \
  --broadcast
```

For detailed instructions, see [DEPLOY.md](DEPLOY.md)

---

## Testing

```bash
# Run all tests
forge test

# Run with gas report
forge test --gas-report

# Run specific test
forge test --match-test testVerifyProof -vvv

# Fork testing (requires RPC_URL)
forge test --fork-url $RPC_URL
```

---

## Architecture

```
┌──────────────────────────────────────────────┐
│            ZeroProof Entry Point              │
│   verifyProof(proofType, proof, claim)       │
└──────────────────┬───────────────────────────┘
                   │
         ┌─────────┴──────────┐
         │                    │
         ▼                    ▼
┌─────────────────┐  ┌──────────────────┐
│  SP1 Verifier   │  │ Reclaim Verifier │
│  (zkVM proofs)  │  │  (zkTLS proofs)  │
└─────────────────┘  └──────────────────┘
```

### Proof Types

- **`sp1-zkvm`**: Zero-knowledge virtual machine proofs
  - Use case: Open-source logic (pricing calculations)
  - Trust: Fully trustless (cryptographic proof)
  - Speed: Slow (11-27 min with GPU)

- **`reclaim-zktls`**: TLS attestation proofs
  - Use case: Proprietary APIs (booking confirmations)
  - Trust: Witness attestation
  - Speed: Fast (~seconds)

---

## Contract Overview

### ZeroProof.sol

Main entry point contract with:
- Multi-proof verification support
- Verifier registry (extensible)
- Standardized claim structure
- Event logging

**Key Functions**:
```solidity
function verifyProof(
    bytes32 proofType,
    bytes calldata proof,
    Claim calldata claim
) external returns (bool)

function registerVerifier(
    bytes32 proofType,
    address verifier
) external onlyOwner
```

### Interfaces

- `ISP1Verifier.sol`: Interface for SP1 universal verifier
- `IReclaimVerifier.sol`: Interface for Reclaim Protocol verifier

---

## Development

### Build

```bash
forge build
```

### Format

```bash
forge fmt
```

### Gas Snapshots

```bash
forge snapshot
```

### Clean

```bash
forge clean
```

---

## Verify on Etherscan

```bash
forge verify-contract \
  <DEPLOYED_ADDRESS> \
  src/ZeroProof.sol:ZeroProof \
  --chain-id 11155111 \
  --etherscan-api-key $ETHERSCAN_API_KEY \
  --constructor-args $(cast abi-encode "constructor(address,address)" $SP1_VERIFIER_ADDRESS $RECLAIM_VERIFIER_ADDRESS)
```

---

## Security

- ✅ OpenZeppelin Ownable for access control
- ✅ Verifier address validation on registration
- ✅ Event logging for all verifications
- ✅ No external calls in verification path
- ⚠️  Owner key must be secured (use multi-sig in production)

---

## License

MIT

---

## Support

- 📖 [Deployment Guide](DEPLOY.md)
- 🔗 [SP1 Docs](https://docs.succinct.xyz/)
- 🔗 [Reclaim Docs](https://docs.reclaimprotocol.org/)
