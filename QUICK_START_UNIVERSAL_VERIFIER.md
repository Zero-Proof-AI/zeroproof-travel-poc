# Quick Start - Universal Verifier Integration

## Overview

Multi-agent system with SP1 v5.2.4 universal verifier:
- ✅ One pre-deployed contract verifies ALL programs
- ✅ GPU-accelerated proof generation
- ✅ On-chain verification working on Sepolia
- ✅ **Agent independence via zk-protocol library** (no cross-dependencies)

## Project Structure

```
zeroproof-travel-poc/
├── agent-a/              # Consumer (calls Agent B, verifies on-chain)
├── agent-b/              # Provider (pricing + booking)
├── zk-attestation-service/  # Proof generation (GPU-accelerated)
├── zk-protocol/          # Shared library (common types, no agent dependencies)
├── sp1-verifier-deploy/  # Foundry project for deploying verifiers
├── README.md
├── ARCHITECTURE.md
└── QUICK_START_UNIVERSAL_VERIFIER.md
```

**Key Design**: `zk-protocol` enables agent independence by providing shared types (`AttestRequest`, `AttestResponse`, `AgentResponse`) without requiring agents to depend on each other's code.

## Deployed Verifier

**Sepolia Testnet:**
- Address: `0x53A9038dCB210D210A7C973fA066Fd2C50aa8847`
- VERIFIER_HASH: `0xa4594c59bbc142f3b81c3ecb7f50a7c34bc9af7c4c444b5d48b795427e285913`
- Version: SP1 v5.2.4

## Running the System

**First Time Setup:** On first run, SP1 will auto-download ~4GB of circuit files to `~/.sp1/circuits/`. This takes 5-10 minutes and only happens once.

### Terminal 1: Attestation Service (GPU-accelerated)
```bash
cd zk-attestation-service/attester
cargo run --release
# Output: ZK Attester running → http://0.0.0.0:8000
```

### Terminal 2: Agent B (Multi-function Provider)
```bash
cd agent-b
cargo run --release --bin agent-b-server
# Output: ✓ ELF registered with attester
#         ✓ Agent B running on http://0.0.0.0:8001
```

### Terminal 3: Agent A (Consumer + Verification)
```bash
cd agent-a
cargo run --release

# Expected output:
# → Calling Agent B at http://localhost:8001
# ✓ Agent B response: {"price":578.0}
# → Requesting attestation from http://localhost:8000
# ✓ Attestation response (STARK phase: 11-27 min)
# ✅ Off-chain proof verified!
# → Verifying proof on-chain at 0x53A9038dCB210D210A7C973fA066Fd2C50aa8847
# ✅ On-chain verification SUCCESS!
```

## What's New

### 1. AttestResponse now includes `vk_hash`
```rust
{
  "proof": "0x...",
  "verified_output": 578.0,
  "success": true,
  "vk_hash": "0x..."  // ← NEW: 32-byte hash for on-chain
}
```

### 2. On-chain verification uses universal verifier
```bash
# Before (deprecated)
export VERIFIER_CONTRACT=0x...

# After (new)
export SP1_VERIFIER_ADDRESS=0x...
```

### 3. Function changed
```solidity
// Before
Verifier.verify(bytes proof, bytes publicInputs) → bool

// After (Universal)
SP1VerifierGroth16.verifyProof(bytes32 vkHash, bytes proof) → bool
```

## Key Points

| Aspect | Value |
|--------|-------|
| Attester Port | 8000 |
| Agent B Port | 8001 |
| SP1 Version | 5.2.4 |
| Proof Format | 260 bytes (4-byte VERIFIER_HASH + 256-byte Groth16) |
| VK Hash Format | 32-byte hex string |
| Universal Verifier (Sepolia) | `0x53A9038dCB210D210A7C973fA066Fd2C50aa8847` |
| STARK Phase | 11-27 minutes (GPU-accelerated) |
| Groth16 Phase | <1 minute |

## What Changed from v5.0.0

### ✅ Working Now
1. **Universal Verifier Deployed**: Custom v5.2.4 contract on Sepolia
2. **VERIFIER_HASH Matches**: `0xa4594c59...` in both proof and contract
3. **On-Chain Verification**: Working end-to-end
4. **Error Handling**: Proper revert detection in agent-a

## Environment Variables

All services have sensible defaults. Override if needed:

```bash
# Agent A (optional overrides)
export AGENT_B_URL=http://localhost:8001
export ATTESTER_URL=http://localhost:8000
export SP1_VERIFIER_ADDRESS=0x53A9038dCB210D210A7C973fA066Fd2C50aa8847
export RPC_URL=<your-sepolia-rpc>

# Agent B (optional)
export ATTESTER_URL=http://localhost:8000

# Attester (runs without env vars)
# No configuration needed - GPU auto-detected
```

## Monitoring Proof Generation

The STARK phase takes 11-27 minutes on RTX 4090. Monitor progress:

```bash
# Watch attester logs
tail -f /tmp/attester.log

# Check GPU usage
nvidia-smi

# Monitor Docker container
docker ps | grep sp1-gnark
```

## Output Example

```
→ Calling Agent B at http://localhost:8001
✓ Agent B response:
  data: {"price":578.0}
  program_id: 89456604-93dd-4aa5-bf70-109367ef33ad
  elf_hash: 0x8e93c12a...

→ Requesting attestation from http://localhost:8000
✓ Attestation response:
  verified_output: {"price":578.0}
  vk_hash: 0x003a2082...
  proof (first 66 chars): a4594c590505eb62...
✅ Off-chain proof verified!

→ Verifying proof on-chain with SP1VerifierGroth16 at 0x53A9038dCB210D210A7C973fA066Fd2C50aa8847
  VK Hash: 0x003a2082...
  Public Values (12 bytes): 000000000000000000108240...
  Proof (260 bytes / 520 hex): a4594c590505eb62...
✓ On-chain verification result: valid ✅
  Call succeeded without revert - proof cryptographically verified!

✅ On-chain verification SUCCESS! Response data is cryptographically valid.
   Verified data: {"price":578.0}
```

## Troubleshooting

### "Connection refused" to Agent B
→ Agent B not running. Start it: `cd agent-b && cargo run --release --bin agent-b-server`

### "Connection refused" to Attester  
→ Attester not running. Start it: `cd zk-attestation-service/attester && cargo run --release`

### "Unknown program_id"
→ Start Agent B first - it registers the ELF on startup

### On-chain verification fails
→ Check contract address: `0x53A9038dCB210D210A7C973fA066Fd2C50aa8847` (Sepolia)
→ Verify RPC URL is correct and accessible

### GPU not being used
→ Check CUDA: `nvidia-smi`
→ Ensure docker has GPU access: `docker run --rm --gpus all nvidia/cuda:12.0.0-base-ubuntu22.04 nvidia-smi`

## Deploy Your Own Verifier

If you need to deploy to a different network:

```bash
cd sp1-verifier-deploy
cp .env.example .env
# Edit .env with your RPC_URL, PRIVATE_KEY, ETHERSCAN_API_KEY
source .env
forge script script/DeploySP1Verifier.s.sol:DeploySP1Verifier \
    --rpc-url $RPC_URL --broadcast --verify
```

See `sp1-verifier-deploy/DEPLOY.md` for details.

---

**Status**: ✅ Fully working end-to-end  
**Components**: 3 services (Attester, Agent B, Agent A)  
**Verification**: On-chain working on Sepolia
