# System Architecture

## Overview

Multi-agent zero-knowledge proof system with universal verification:

```
┌─────────────────────────────────────────────────────────────────┐
│                    Agent A (Consumer)                           │
│  Requests service → Gets proof → Verifies on-chain              │
└────────┬──────────────────────────────────────┬─────────────────┘
         │                                      │
         ▼                                      ▼
┌──────────────────────┐                ┌──────────────────────┐
│   Agent B Server     │                │  Sepolia Testnet     │
│  - Pricing service   │                │  Universal Verifier  │
│  - Booking service   │                │  0x53A9038dCB210...  │
│  - ELF registration  │                └──────────────────────┘
└──────────┬───────────┘                          ▲
           │                                      │
           ▼                                      │
┌──────────────────────────────────────────────────────────┐
│    ZK Attester Service (GPU-accelerated)                 │
│ - Receives ELF from Agent B                              │
│ - STARK proof generation (GPU, 11-27 min)                │
│ - Groth16 proof generation (<1 min)                      │
│ - Returns: proof + vk_hash + public_values               │
└──────────────────────────────────────────────────────────┘
```

## Component Details

### 1. Agent A (Consumer)

**Location**: `/agent-a/`

**Purpose**: Consumes Agent B services with cryptographic verification

**Flow**:
```
1. HTTP POST http://localhost:8001/price
   ├─ Request: { from: "NYC", to: "LON", vip: true }
   ├─ Response: { data: {"price":578.0}, program_id: uuid, elf_hash: 0x... }
   └─ Store: program_id

2. HTTP POST http://localhost:8001/zk-input
   ├─ Request: { endpoint: "price", input: {...} }
   ├─ Response: { input_bytes: [1,2,3...] }
   └─ Get properly formatted zkVM input

3. HTTP POST http://localhost:8000/attest
   ├─ Payload: { program_id, input_bytes, claimed_output, verify_locally: true }
   ├─ Wait: 11-27 minutes (STARK) + <1 min (Groth16)
   ├─ Response: { proof: 0x..., vk_hash: 0x..., public_values: 0x..., verified_output: 578.0 }
   └─ Verify: local verification passed

4. eth_call to Sepolia (JSON-RPC)
   ├─ Contract: SP1VerifierGroth16 at 0x53A9038dCB210D210A7C973fA066Fd2C50aa8847
   ├─ Method: verifyProof(bytes32 vkHash, bytes publicValues, bytes proof)
   ├─ Response: Success (no revert) or Error (revert with reason)
   └─ Success: cryptographically verified on-chain!
```

**Environment Variables**:
- `AGENT_B_URL`: Agent B endpoint (default: http://localhost:8001)
- `ATTESTER_URL`: Attester endpoint (default: http://localhost:8000)
- `SP1_VERIFIER_ADDRESS`: Universal verifier contract (default: 0x53A9038dCB210D210A7C973fA066Fd2C50aa8847)
- `RPC_URL`: Sepolia RPC endpoint
- `RPC_URL`: Libertas RPC endpoint (optional; if missing, skips on-chain verify)

**Key Code**:
- Main loop: waits for user input, calls Agent B, attester, contract
- `verify_on_chain()`: encodes proof + inputs, calls contract via JSON-RPC

**Dependencies**:
- reqwest (HTTP client)
- serde_json (JSON parsing)
- ethers::abi (ABI encoding)
- hex (hex encoding/decoding)

---

### 2. Agent B Server (Multi-function Provider)

**Location**: `/agent-b/`

**Purpose**: Multi-function service (pricing + booking) with zkVM proof support

**Startup Flow**:
```
1. On startup:
   ├─ Read ELF from target/elf-compilation/.../agent-b-program
   ├─ POST to attester at /register-elf
   │  ├─ File: ELF binary
   │  ├─ Response: { program_id: uuid, elf_hash: 0x... }
   └─ Store program_id

2. Start HTTP server on 0.0.0.0:8001
```

**Endpoints**:

**POST /price**
```json
Request: { "from": "NYC", "to": "LON", "vip": true }
Response: {
  "data": {"price": 578.0},
  "program_id": "89456604-93dd-4aa5-bf70-109367ef33ad",
  "elf_hash": "0x8e93c12ab6da873e..."
}
```

**POST /zk-input**
```json
Request: { "endpoint": "price", "input": {...} }
Response: { "input_bytes": [1, 2, 3, ...] }
Purpose: Returns properly formatted bincode bytes for zkVM
```

**POST /book** (future)
```json
Request: { "from": "NYC", "to": "LON", "date": "2025-12-20" }
Response: { "data": {"confirmation": "ABC123"}, "program_id": "...", "elf_hash": "..." }
```

**Environment Variables**:
- `ATTESTER_URL`: Attester location (default: http://localhost:8000)
- `BOOKING_API_URL`: External booking API (optional)

**Key Features**:
- Single ELF handles multiple RPC functions (pricing, booking)
- Agent A doesn't need to know internal zkVM structure
- Each function returns its own proof

---

### 3. ZK Attester Service (GPU-accelerated Proof Generator)

**Location**: `/zk-attestation-service/attester/`

**Purpose**: Generates SP1 v5.2.4 proofs with GPU acceleration

**Startup Flow**:
```
1. Initialize in-memory HashMap for ELF storage
2. Detect GPU (NVIDIA CUDA)
3. Start HTTP server on 0.0.0.0:8000
```

**Endpoints**:

**POST /register-elf** (multipart/form-data)
```
Request (multipart):
  - file: ELF binary
  - field: elf_name (optional)

Response:
{
  "program_id": "89456604-93dd-4aa5-bf70-109367ef33ad",
  "elf_hash": "0x8e93c12ab6da873e..."
}
```

**POST /attest** (application/json)
```
Request:
{
  "program_id": "89456604-93dd-4aa5-bf70-109367ef33ad",
  "input_bytes": [1, 2, 3, ...],
  "claimed_output": "{\"price\":578.0}",
  "verify_locally": true
}

Response:
{
  "success": true,
  "proof": "0xa4594c59bbc142f3...",  // 260 bytes (VERIFIER_HASH + Groth16)
  "public_values": "0x000000000000000000108240",  // 12 bytes
  "vk_hash": "0x003a20824d4b95530548ffa351cb96699dc3ed7386719ab90699d49dd910273c",
  "verified_output": "{\"price\":578.0}"
}
```

**Proof Generation Pipeline**:
```
1. Retrieve ELF from HashMap by program_id
2. Create SP1 ProverClient (GPU-accelerated)
3. Call prover.setup(&elf) → get proving key (PK) and verifying key (VK)
4. Compute vk_hash = vk.bytes32()
5. STARK Phase (GPU-accelerated):
   ├─ Create stdin with input_bytes
   ├─ prover.prove(&pk, &stdin) → generates STARK proof
   └─ Uses CUDA for acceleration (1000-3000% CPU usage = multi-core + GPU)
6. Groth16 Phase (<1 minute):
   ├─ .groth16().run() → wraps STARK in Groth16
   ├─ Uses Docker container sp1-gnark
   └─ Result: 260-byte proof (4-byte VERIFIER_HASH + 256-byte Groth16)
7. Local Verification:
   ├─ prover.verify(&proof, &vk)
   └─ Ensures proof is valid before returning
8. Extract components:
   ├─ proof_bytes = proof.bytes()  // 260 bytes with VERIFIER_HASH
   ├─ public_values = proof.public_values.as_slice()
   └─ vk_hash (32 bytes)
9. Return AttestResponse
```

**No Environment Variables Required**
- GPU auto-detected via CUDA
- All computation local, no blockchain interaction

**Key Features**:
- GPU acceleration for STARK phase
- Docker-based Groth16 wrapping
- Universal proof format (works with any v5.2.4 verifier)
- Local verification before returning

**Dependencies**:
- sp1-sdk v5.2.4 (proof generation)
- axum (HTTP server)
- tokio (async runtime)
- serde/serde_json (serialization)
- hex (encoding)

---

### 4. Universal Verifier Contract (On-Chain Verification)

**Deployed**: Sepolia at `0x53A9038dCB210D210A7C973fA066Fd2C50aa8847`

**Purpose**: Verifies ALL SP1 v5.2.4 Groth16 proofs (program-agnostic, version-specific)

