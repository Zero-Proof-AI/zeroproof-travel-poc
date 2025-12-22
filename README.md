# ZeroProof Travel PoC - Zero-Knowledge Proof Attestation System

A complete end-to-end system for generating cryptographic proofs of computation and verifying them on-chain using SP1 zkVM with universal Groth16 verification.

## What is This?

Multi-agent system with zero-knowledge proof attestation:
1. **Agent B**: Multi-function service (pricing + booking) with zkVM proof generation
2. **Attestation Service**: GPU-accelerated STARK + Groth16 proof generation using SP1 v5.2.4
3. **Agent A**: Consumer that verifies proofs on-chain using universal verifier
4. **Universal Verifier**: One deployed contract verifies ALL zkVM programs

## Quick Start

```bash
# Terminal 1: Start attestation service (GPU-accelerated)
cd zk-attestation-service/attester
cargo run --release

# Terminal 2: Start Agent B (pricing + booking)
cd agent-b
cargo run --release --bin agent-b-server

# Terminal 3: Run Agent A (consumer + on-chain verification)
cd agent-a
cargo run --release
```

**Expected Output:**
- ✅ Agent B responds with pricing data
- ✅ Attester generates STARK proof (11-27 min on RTX 4090)
- ✅ Attester generates Groth16 proof (fast)
- ✅ Local verification passes
- ✅ On-chain verification succeeds on Sepolia

**Note:** On first run, SP1 will auto-download ~4GB of circuit files to `~/.sp1/circuits/`. This takes 5-10 minutes and only happens once.

## On-Chain Verification

**Deployed Universal Verifier (Sepolia):**
- Address: `0x53A9038dCB210D210A7C973fA066Fd2C50aa8847`
- Version: SP1 v5.2.4
- VERIFIER_HASH: `0xa4594c59bbc142f3b81c3ecb7f50a7c34bc9af7c4c444b5d48b795427e285913`

This single contract verifies proofs from ALL programs using SP1 v5.2.4.

## Documentation

- **`QUICK_START_UNIVERSAL_VERIFIER.md`** - Complete setup guide
- **`ARCHITECTURE.md`** - System design
- **`sp1-verifier-deploy/DEPLOY.md`** - How to deploy your own verifier

## Components

- **Agent B** (`/agent-b/`) - Multi-function service (pricing + booking)
- **Agent A** (`/agent-a/`) - Consumer with on-chain verification
- **Attester** (`/zk-attestation-service/attester/`) - GPU-accelerated proof generator
- **zk-protocol** (`/zk-protocol/`) - Shared library for agent independence (common types: `AttestRequest`, `AttestResponse`, `AgentResponse`)
- **Universal Verifier** (Sepolia: `0x53A9038dCB210D210A7C973fA066Fd2C50aa8847`) - On-chain verification
- **sp1-verifier-deploy** (`/sp1-verifier-deploy/`) - Foundry project for deploying custom verifiers

## Tech Stack

- **SP1 v5.2.4** - Zero-knowledge VM with Groth16 proving
- **Rust** - All services
- **GPU** - NVIDIA CUDA for STARK phase acceleration
- **Sepolia** - Ethereum testnet for verification

## Features

✅ Multi-function independent agents  
✅ GPU-accelerated proof generation (RTX 4090)  
✅ Universal verifier (one contract for all programs)  
✅ On-chain verification via JSON-RPC  
✅ End-to-end integration working  
✅ STARK + Groth16 two-phase proving

## Architecture

```
Agent A (Consumer)
    ↓ /price request
Agent B (Provider) → Returns price: 578.0
    ↓ /zk-input request
Agent B → Returns bincode bytes
    ↓ /attest request
Attester:
  - STARK proof (GPU, 11-27 min)
  - Groth16 proof (fast)
  - Local verification ✅
    ↓ proof + vk_hash
Agent A → On-chain verification ✅
    ↓ eth_call
Universal Verifier (Sepolia) → Success!
```

## Performance

- **STARK phase**: 11-27 minutes (GPU-accelerated)
- **Groth16 phase**: <1 minute
- **On-chain gas**: ~250k gas per verification
- **Proof size**: 260 bytes

---
