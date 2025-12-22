# ZeroProof Contract Deployment Guide

## Quick Deploy to Sepolia

### Prerequisites

1. **Install Foundry** (if not already installed):
```bash
curl -L https://foundry.paradigm.xyz | bash
foundryup
```

2. **Get Sepolia ETH**: Use a faucet like [Alchemy Sepolia Faucet](https://sepoliafaucet.com/)

3. **Get API Keys**:
   - Sepolia RPC: [Alchemy](https://www.alchemy.com/) or [Infura](https://www.infura.io/)
   - Etherscan API: [Etherscan](https://etherscan.io/myapikey)

---

## Step-by-Step Deployment

### 1. Setup Environment

```bash
cd /home/revolution/zeroproof-travel-poc/contracts

# Create .env file
cat > .env << 'EOF'
# Sepolia RPC URL
RPC_URL=https://eth-sepolia.g.alchemy.com/v2/YOUR_ALCHEMY_KEY

# Your deployer private key (NEVER COMMIT THIS!)
PRIVATE_KEY=0xYOUR_PRIVATE_KEY_HERE

# Etherscan API key for contract verification
ETHERSCAN_API_KEY=YOUR_ETHERSCAN_API_KEY

# Pre-deployed verifiers on Sepolia
SP1_VERIFIER_ADDRESS=0x53A9038dCB210D210A7C973fA066Fd2C50aa8847
RECLAIM_VERIFIER_ADDRESS=0xAe94FB09711e1c6B057853a515483792d8e474d0
EOF
```

**⚠️ Security**: Never commit `.env` to git! It's already in `.gitignore`.

### 2. Install Dependencies

```bash
# Install OpenZeppelin contracts
forge install OpenZeppelin/openzeppelin-contracts

# Install forge-std (test utilities)
forge install foundry-rs/forge-std
```

### 3. Build Contracts

```bash
# Compile all contracts
forge build

# Check for compilation errors
forge build --sizes
```

Expected output:
```
[⠊] Compiling...
[⠒] Compiling 43 files with Solc 0.8.24
[⠢] Solc 0.8.24 finished in 226.14ms
Compiler run successful!
```

### 4. Run Tests (Optional but Recommended)

```bash
# Run all tests
forge test

# Run with verbosity to see details
forge test -vvv

# Run specific test
forge test --match-test testVerifyProof -vvv
```

### 5. Deploy ZeroProof Contract

**Option A: Using Forge Script (Recommended)**

```bash
# Load environment variables
source .env

# Deploy to Sepolia
forge script script/DeployZeroProof.s.sol:DeployZeroProof \
  --rpc-url $RPC_URL \
  --private-key $PRIVATE_KEY \
  --broadcast \
  -vv
```

**Option B: Manual Deploy + Verify**

```bash
# Deploy only (without verification)
forge create src/ZeroProof.sol:ZeroProof \
  --rpc-url $RPC_URL \
  --private-key $PRIVATE_KEY \
  --constructor-args $SP1_VERIFIER_ADDRESS $RECLAIM_VERIFIER_ADDRESS

# Note the deployed address from output, then verify:
forge verify-contract \
  <DEPLOYED_ADDRESS> \
  src/ZeroProof.sol:ZeroProof \
  --chain-id 11155111 \
  --etherscan-api-key $ETHERSCAN_API_KEY \
  --constructor-args $(cast abi-encode "constructor(address,address)" $SP1_VERIFIER_ADDRESS $RECLAIM_VERIFIER_ADDRESS)
```

### 6. Verify Deployment

After deployment, you'll see output like:
```
=== Deployment Complete ===
ZeroProof deployed at: 0x9C33252D29B41Fe2706704a8Ca99E8731B58af41
```

**Verify on Etherscan**:
1. Go to: `https://sepolia.etherscan.io/address/YOUR_DEPLOYED_ADDRESS`
2. Check "Contract" tab - should show green checkmark ✅
3. Try "Read Contract" to see verifiers registered

### 7. Update Environment Files

```bash
# Add deployed address to contracts/.env
echo "ZEROPROOF_ADDRESS=YOUR_DEPLOYED_ADDRESS" >> .env

# Update Agent A configuration
cat > ../agent-a/.env << EOF
AGENT_B_URL=http://localhost:8001
ATTESTER_URL=http://localhost:8000
ZEROPROOF_ADDRESS=YOUR_DEPLOYED_ADDRESS
RPC_URL=$RPC_URL
EOF
```

---

## Testing the Deployment

### 1. Check Registered Verifiers

```bash
# Read SP1 verifier address
cast call YOUR_ZEROPROOF_ADDRESS \
  "verifiers(bytes32)(address)" \
  $(cast keccak "sp1-zkvm") \
  --rpc-url $RPC_URL

# Read Reclaim verifier address
cast call YOUR_ZEROPROOF_ADDRESS \
  "verifiers(bytes32)(address)" \
  $(cast keccak "reclaim-zktls") \
  --rpc-url $RPC_URL
```

### 2. Check Owner

```bash
cast call YOUR_ZEROPROOF_ADDRESS \
  "owner()(address)" \
  --rpc-url $RPC_URL
```

### 3. Register Additional Verifier (Owner Only)

```bash
# Example: Register a new proof type
cast send YOUR_ZEROPROOF_ADDRESS \
  "registerVerifier(bytes32,address)" \
  $(cast keccak "plonk") \
  0xYOUR_PLONK_VERIFIER_ADDRESS \
  --rpc-url $RPC_URL \
  --private-key $PRIVATE_KEY
```

---

## Common Issues & Solutions

### Issue: "Failed to decode private key"

**Solution**: Make sure private key starts with `0x` and is 64 hex characters (32 bytes)
```bash
# Check private key format
echo $PRIVATE_KEY | wc -c  # Should be 67 (including 0x and newline)
```

### Issue: "Insufficient funds"

**Solution**: Get Sepolia ETH from faucet
```bash
# Check your balance
cast balance YOUR_ADDRESS --rpc-url $RPC_URL
```

### Issue: "Verifier at address(0)"

**Solution**: Make sure verifier addresses are set in `.env`
```bash
# Verify environment variables are loaded
echo $SP1_VERIFIER_ADDRESS
echo $RECLAIM_VERIFIER_ADDRESS
```

### Issue: "Contract verification failed"

**Solution**: Wait 30 seconds after deployment, then retry verification
```bash
sleep 30
forge verify-contract ...
```

---

## Gas Optimization Tips

### 1. Estimate Gas Before Deploy

```bash
# Dry run deployment (no broadcast)
forge script script/DeployZeroProof.s.sol:DeployZeroProof \
  --rpc-url $RPC_URL
```

### 2. Use Gas Price Estimator

```bash
# Check current gas price
cast gas-price --rpc-url $RPC_URL

# Deploy with specific gas price (in wei)
forge script script/DeployZeroProof.s.sol:DeployZeroProof \
  --rpc-url $RPC_URL \
  --private-key $PRIVATE_KEY \
  --broadcast \
  --gas-price 2000000000  # 2 gwei
```

### 3. Deploy to Testnet First

Always test on Sepolia before mainnet:
- Sepolia: `--chain-id 11155111`
- Ethereum Mainnet: `--chain-id 1`

---

## Production Deployment Checklist

Before deploying to mainnet:

- [ ] All tests pass: `forge test`
- [ ] Gas costs reviewed: `forge snapshot`
- [ ] Code audited (for production)
- [ ] Verifier addresses confirmed on target chain
- [ ] Sufficient ETH for deployment (~0.01 ETH recommended)
- [ ] Private key secured (use hardware wallet for mainnet)
- [ ] Multi-sig owner for production (not single EOA)
- [ ] Emergency pause mechanism considered
- [ ] Documentation updated

---

## Useful Forge Commands

```bash
# View contract size
forge build --sizes

# Generate gas snapshot
forge snapshot

# Format Solidity code
forge fmt

# Run security checks
forge lint

# Clean build artifacts
forge clean

# Update dependencies
forge update

# Run fork tests (simulate on real chain state)
forge test --fork-url $RPC_URL

# Interactive debugger
forge test --debug testVerifyProof
```

---

## Multi-Chain Deployment

Deploy to multiple networks using the same script:

### Optimism Sepolia
```bash
RPC_URL=https://sepolia.optimism.io \
SP1_VERIFIER_ADDRESS=0x... \
RECLAIM_VERIFIER_ADDRESS=0x... \
forge script script/DeployZeroProof.s.sol:DeployZeroProof \
  --rpc-url $RPC_URL \
  --private-key $PRIVATE_KEY \
  --broadcast
```

### Base Sepolia
```bash
RPC_URL=https://sepolia.base.org \
SP1_VERIFIER_ADDRESS=0x... \
RECLAIM_VERIFIER_ADDRESS=0xF90085f5Fd1a3bEb8678623409b3811eCeC5f6A5 \
forge script script/DeployZeroProof.s.sol:DeployZeroProof \
  --rpc-url $RPC_URL \
  --private-key $PRIVATE_KEY \
  --broadcast
```

### Polygon Amoy
```bash
RPC_URL=https://rpc-amoy.polygon.technology \
SP1_VERIFIER_ADDRESS=0x... \
RECLAIM_VERIFIER_ADDRESS=0xcd94A4f7F85dFF1523269C52D0Ab6b85e9B22866 \
forge script script/DeployZeroProof.s.sol:DeployZeroProof \
  --rpc-url $RPC_URL \
  --private-key $PRIVATE_KEY \
  --broadcast
```

See [Reclaim Networks](https://docs.reclaimprotocol.org/onchain/solidity/supported-networks) for all Reclaim verifier addresses.

---

## Deployment Artifacts

After deployment, artifacts are saved in:

- **Broadcast logs**: `broadcast/DeployZeroProof.s.sol/<chain-id>/run-latest.json`
- **Compiled contracts**: `out/ZeroProof.sol/ZeroProof.json`
- **Gas snapshots**: `.gas-snapshot`

**Keep these safe** - they contain deployment details and transaction hashes.

---

## Support

- **Forge Documentation**: https://book.getfoundry.sh/
- **SP1 Docs**: https://docs.succinct.xyz/
- **Reclaim Docs**: https://docs.reclaimprotocol.org/
- **Issues**: Create an issue in the repository

---

**Last Updated**: December 16, 2025  
**Network**: Sepolia Testnet  
**ZeroProof Address**: `0x9C33252D29B41Fe2706704a8Ca99E8731B58af41`
