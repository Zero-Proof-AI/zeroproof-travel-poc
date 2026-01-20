# System Architecture

## Overview

Multi-agent zero-knowledge proof system with universal verification:

```
┌─────────────────────────────────────────────────────────────────────────┐
│                  Claude AI (Orchestration)                              │
│         Decides workflow, interprets results, handles errors             │
└────────┬──────────────────────────────────────────────┬──────────────────┘
         │                                              │
         ▼                                              ▼
┌──────────────────────┐         ┌──────────────────────────────┐
│ Agent A MCP Server   │         │  Python MCP Client          │
│ (Rust - High Perf)   │◄───────►│  (Claude Integration)       │
│ - call_agent_b       │         │  - agent-service package    │
│ - format_zk_input    │         │  - Shared Agent core        │
│ - request_attestation│         └──────────────────────────────┘
│ - verify_on_chain    │
└──────────┬───────────┘
           │ (HTTP Requests)
    ┌──────┴────────┬──────────────────┬──────────────────┐
    ▼               ▼                  ▼                  ▼
┌────────────┐ ┌────────────┐  ┌──────────────┐  ┌──────────────┐
│ Agent B    │ │ Attester   │  │ ZeroProof    │  │ Sepolia      │
│ Server     │ │ Service    │  │ Contract     │  │ Testnet      │
│ - Pricing  │ │ - STARK    │  │ - Verifier   │  │ - RPC        │
│ - Booking  │ │ - Groth16  │  │   (15+ min)  │  │   Endpoint   │
│ - ELF Reg  │ │   (GPU)    │  │              │  │              │
└────────────┘ └────────────┘  └──────────────┘  └──────────────┘
```

## Component Details

### 1. Agent A - Rust MCP Server

**Location**: `/agent-a/mcp-server/`

**Technology**: Rust + rmcp SDK (Model Context Protocol)

**Purpose**: Exposes ZK operations as MCP tools for Claude orchestration

**Architecture**:
- `src/lib.rs`: Core reusable functions (verify_on_chain, call_agent_b, etc.)
- `src/main.rs`: MCP server & tool wrappers using `#[tool_router]` macro
- Stdio transport (works with Claude Desktop, MCP Inspector, Python clients)

**Tools Exposed**:

1. **call_agent_b**: Get pricing and program ID from Agent B
   ```
   Input: { from: "NYC", to: "LON", vip: true }
   Output: { price: 578.0, program_id: uuid, elf_hash: 0x... }
   ```

2. **format_zk_input**: Format input for zkVM computation
   ```
   Input: { endpoint: "price", input: {...} }
   Output: { input_hex: "0x...", input_array: [...], length: 256 }
   ```

3. **request_attestation**: Request ZK proof from attester (⏱️ 11-27 min)
   ```
   Input: { program_id, input_hex, claimed_output }
   Output: { proof, public_values, vk_hash, verified_output }
   ```

4. **verify_on_chain**: Verify proof on Sepolia blockchain
   ```
   Input: { proof, public_values, vk_hash }
   Output: { verified: true/false, details: "..." }
   ```

**Flow**:
```
Claude → MCP Protocol (stdio) → Rust Server → HTTP Calls → Agent B/Attester/Blockchain

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
- `ZEROPROOF_ADDRESS`: ZeroProof contract on Sepolia (default: 0x9C33252D29B41Fe...)
- `RPC_URL`: Sepolia RPC endpoint for verification

**Key Features**:
- Performance: 100% Rust implementation for speed
- Type Safety: Schemars validates tool parameters as JSON schema
- Reusable: Original Rust logic extracted into library functions
- Flexible: Works standalone or orchestrated by Claude

**Dependencies**:
- rmcp (Rust MCP SDK with macros)
- tokio (async runtime)
- ethers (ABI encoding)
- reqwest (HTTP client)
- serde/schemars (serialization + schema)

---

### 2. Agent A - Python MCP Client

**Location**: `/agent-service/mcp_client/agent_a/`

**Technology**: Python + Anthropic Claude API

**Purpose**: Provides intelligent orchestration via Claude for Agent A Rust MCP server

**Architecture**:
- Uses `shared/agent/core.py`: Agent class with MCP client support
- Launches Rust MCP server as subprocess via stdio transport
- Claude interprets tool descriptions and decides workflow

**Capabilities**:
- Interactive chat interface
- Automatic tool calling based on Claude's reasoning
- Error recovery and retry logic
- Natural language workflow orchestration

**Example Conversation**:
```
User: "Get pricing from NYC to London for a VIP customer"

Claude: "I'll help you get pricing and generate a cryptographic proof.
Step 1: Calling Agent B for pricing..."
[Claude calls: call_agent_b(from='NYC', to='London', vip=true)]

Result: Price is $578, program_id is 'abc-123', elf_hash is '0x...'

Step 2: Formatting ZK input..."
[Claude calls: format_zk_input(endpoint='price', input={...})]

Result: Input formatted to 256 bytes

Step 3: This will take ~15 minutes for proof generation. Requesting attestation..."
[Claude calls: request_attestation(program_id='abc-123', input_hex='0x...')]

[Waiting 15 minutes...]

Result: Proof generated! Got public_values and vk_hash.

Step 4: Verifying on Sepolia blockchain..."
[Claude calls: verify_on_chain(proof='0x...', public_values='0x...', vk_hash='0x...')]

Result: ✅ Verified! The pricing data ($578) is cryptographically valid on-chain."
```

---

### 3. Agent B Server (Multi-function Provider)

**Location**: `/agent-b/`

**Technology**: Rust

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
  "data": "578.0",
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

