import { useState } from 'react';
import { useConnect, useConnection } from 'wagmi';
import { ethers } from 'ethers';
import type { FullProofData } from './ProofModal';

export const useProofVerification = () => {
  const { connect, connectors } = useConnect();
  const { address, isConnected } = useConnection();
  const [isVerifying, setIsVerifying] = useState(false);
  const [verifiedProofIds, setVerifiedProofIds] = useState<Set<string>>(new Set());
  const [verificationError, setVerificationError] = useState<string | null>(null);

  // Check and switch network if needed
  const ensureSepoliaNetwork = async (ethereumProvider: any) => {
    try {
      const provider = new ethers.BrowserProvider(ethereumProvider);
      const network = await provider.getNetwork();
      const chainId = network.chainId;

      console.log('🔗 Current chain ID:', chainId);

      // Sepolia chain ID is 11155111
      if (Number(chainId) === 11155111) {
        console.log('✅ Already on Sepolia');
        return true;
      }

      console.log('⚠️ Wrong network. Chain ID:', chainId);

      // Suggest switching to Sepolia
      const shouldSwitch = window.confirm(
        `⚠️ You are not on Sepolia testnet (current: ${chainId}).\n\n` +
        'Click OK to switch to Sepolia, or cancel to continue anyway.'
      );

      if (shouldSwitch) {
        try {
          // Try to switch to Sepolia
          await ethereumProvider.request({
            method: 'wallet_switchEthereumChain',
            params: [{ chainId: '0xaa36a7' }], // 0xaa36a7 is 11155111 in hex
          });
          console.log('✅ Switched to Sepolia');
          return true;
        } catch (switchError: any) {
          // If chain doesn't exist, add it
          if (switchError.code === 4902) {
            try {
              await ethereumProvider.request({
                method: 'wallet_addEthereumChain',
                params: [
                  {
                    chainId: '0xaa36a7',
                    chainName: 'Sepolia',
                    rpcUrls: ['https://sepolia.infura.io/v3/YOUR_INFURA_KEY'],
                    nativeCurrency: {
                      name: 'Ether',
                      symbol: 'ETH',
                      decimals: 18,
                    },
                    blockExplorerUrls: ['https://sepolia.etherscan.io'],
                  },
                ],
              });
              console.log('✅ Added and switched to Sepolia');
              return true;
            } catch (addError) {
              console.error('Failed to add Sepolia network:', addError);
              return false;
            }
          }
          console.error('Failed to switch network:', switchError);
          return false;
        }
      }

      // User chose to continue anyway
      console.warn('⚠️ Continuing on chain ' + chainId);
      return false;
    } catch (error) {
      console.error('Error checking network:', error);
      return false;
    }
  };

  const handleVerify = async (selectedProof: FullProofData) => {
    if (!isConnected) {
      // Find injected connector and connect
      const injectedConnector = connectors.find(c => c.id === 'injected');
      if (injectedConnector) {
        connect({ connector: injectedConnector });
      }
      return;
    }

    // Wallet is connected - prepare and sign verification transaction
    if (!selectedProof.proof?.onchainProof) {
      console.warn('⚠️ On-chain proof data not available');
      return;
    }

    setIsVerifying(true);
    try {
      // Get provider and user address from window.ethereum
      const ethereumProvider = (window as any).ethereum;
      if (!ethereumProvider) {
        throw new Error('MetaMask not available');
      }

      // Check and ensure Sepolia network
      console.log('🔐 Checking network...');
      const isOnSepolia = await ensureSepoliaNetwork(ethereumProvider);
      if (!isOnSepolia) {
        console.warn('⚠️ Not on Sepolia, but user chose to continue');
      }

      const provider = new ethers.BrowserProvider(ethereumProvider);
      const userAddress = address as string;

      console.log('🔐 Preparing proof verification...');
      console.log('   User address:', userAddress);
      console.log('   Proof ID:', selectedProof.proof_id);

      // Log the onchainProof structure for debugging
      console.log('📋 OnChain Proof Structure:');
      console.log(JSON.stringify(selectedProof.proof.onchainProof, null, 2));

      const onchainProof = selectedProof.proof.onchainProof;
      console.log('🔍 Detailed inspection:');
      console.log('   claimInfo:', onchainProof.claimInfo);
      console.log('   signedClaim:', onchainProof.signedClaim);

      // Extract fields from the nested structure
      const claimInfo = onchainProof.claimInfo || {};
      const signedClaim = onchainProof.signedClaim || {};

      console.log('📦 Extracted fields:');
      console.log('   Claim Info keys:', Object.keys(claimInfo));
      console.log('   Signed Claim keys:', Object.keys(signedClaim));
      console.log('   Full claimInfo:', JSON.stringify(claimInfo, null, 2));
      console.log('   Full signedClaim:', JSON.stringify(signedClaim, null, 2));

      // Extract values from nested structure for contract call
      const claim = signedClaim.claim;

      console.log('✅ Extracted contract parameters:');
      console.log('   identifier:', claim.identifier);
      console.log('   epoch:', claim.epoch);
      console.log('   owner:', claim.owner);
      console.log('   provider:', claimInfo.provider);
      console.log('   signatures count:', signedClaim.signatures.length);

      // Reclaim contract details
      const RECLAIM_ADDRESS = '0xAe94FB09711e1c6B057853a515483792d8e474d0';

      // Encode function call data for verifyProof
      const iface = new ethers.Interface([
        `function verifyProof(
          tuple(
            tuple(string provider, string parameters, string context) claimInfo,
            tuple(
              tuple(bytes32 identifier, address owner, uint32 timestampS, uint32 epoch) claim,
              bytes[] signatures
            ) signedClaim
          ) proof
        ) public`
      ]);

      console.log('🔧 Encoding function data...');
      // Pass the full onchainProof structure with claimInfo and signedClaim
      const proofStructure = [
        [
          claimInfo.provider,
          claimInfo.parameters,
          claimInfo.context,
        ],
        [
          [claim.identifier, claim.owner, claim.timestampS, claim.epoch],
          signedClaim.signatures,
        ],
      ];
      // console.log('   Proof structure:', JSON.stringify(proofStructure, null, 2));

      const encodedData = iface.encodeFunctionData('verifyProof', [proofStructure]);
      console.log('   📝 Encoded function data:', encodedData);

      // Get network info
      const network = await provider.getNetwork();
      const chainId = network.chainId;
      console.log('   🔗 Chain ID:', chainId);

      // Validate network - should be Sepolia (11155111)
      if (Number(chainId) !== 11155111) {
        throw new Error(`Wrong network. Please switch to Sepolia testnet. Current chain ID: ${chainId}`);
      }

      // Get current nonce
      const nonce = await provider.getTransactionCount(userAddress);
      console.log('   🔢 Nonce:', nonce);

      // Get gas price with error handling
      let gasPrice = null;
      let gasLimit = 100000; // Default gas limit

      try {
        const feeData = await provider.getFeeData();
        if (feeData.gasPrice) {
          gasPrice = feeData.gasPrice;
          console.log('   ⛽ Gas price:', gasPrice?.toString());
        }

        // Estimate gas for the call
        const gasEstimate = await provider.estimateGas({
          to: RECLAIM_ADDRESS,
          from: userAddress,
          data: encodedData,
          value: '0',
        });
        gasLimit = Number((BigInt(gasEstimate.toString()) * BigInt(120)) / BigInt(100)); // Add 20% buffer
        console.log('   ⛽ Gas estimate:', gasEstimate.toString());
        console.log('   ⛽ Gas limit:', gasLimit);
      } catch (gasError) {
        console.warn('⚠️ Gas estimation failed, using defaults:', gasError);
      }

      // Create EIP-712 typed data structure for readable signing
      const typedData = {
        types: {
          EIP712Domain: [
            { name: 'name', type: 'string' },
            { name: 'version', type: 'string' },
            { name: 'chainId', type: 'uint256' },
            { name: 'verifyingContract', type: 'address' },
          ],
          VerifyProofTransaction: [
            { name: 'to', type: 'address' },
            { name: 'data', type: 'bytes' },
            { name: 'value', type: 'uint256' },
            { name: 'nonce', type: 'uint256' },
            { name: 'gas', type: 'uint256' },
            { name: 'gasPrice', type: 'uint256' },
          ],
        },
        primaryType: 'VerifyProofTransaction',
        domain: {
          name: 'ReclaimVerifier',
          version: '1',
          chainId: Number(chainId),
          verifyingContract: RECLAIM_ADDRESS,
        },
        message: {
          to: RECLAIM_ADDRESS,
          data: encodedData,
          value: 0,
          nonce: nonce,
          gas: gasLimit.toString(),
          gasPrice: gasPrice?.toString() || '0',
        },
      };

      console.log('📋 EIP-712 Typed Data created');
      console.log('   Domain:', typedData.domain);
      console.log('   Message:', typedData.message);

      // Request user to sign the typed data
      console.log('🦊 Requesting user signature via eth_signTypedData_v4...');

      const userSignature = await ethereumProvider.request({
        method: 'eth_signTypedData_v4',
        params: [userAddress, JSON.stringify(typedData)],
      });

      console.log('✅ User signed the transaction!');
      console.log('   📋 Signature:', userSignature);

      // Build and send raw transaction with signature
      const txObject: any = {
        from: userAddress,
        to: RECLAIM_ADDRESS,
        data: encodedData,
        value: '0x0',
        nonce: ethers.toBeHex(nonce),
        gasLimit: ethers.toBeHex(gasLimit),
      };

      // Add gas price if available
      if (gasPrice) {
        txObject.gasPrice = ethers.toBeHex(gasPrice);
      }

      console.log('🚀 Submitting signed transaction to blockchain...');
      console.log('   Transaction:', txObject);

      // Send transaction directly via eth_sendTransaction
      const txHash = await ethereumProvider.request({
        method: 'eth_sendTransaction',
        params: [txObject],
      });

      console.log('✅ Transaction submitted to blockchain!');
      console.log('   📨 Transaction Hash:', txHash);

      // Wait for transaction confirmation
      console.log('⏳ Waiting for transaction confirmation...');
      let receipt = null;
      let attempts = 0;
      const maxAttempts = 120; // 2 minutes with 1 second intervals

      while (!receipt && attempts < maxAttempts) {
        receipt = await provider.getTransactionReceipt(txHash);
        if (!receipt) {
          await new Promise(resolve => setTimeout(resolve, 1000));
          attempts++;
        }
      }

      if (!receipt) {
        throw new Error('Transaction not confirmed after 2 minutes');
      }

      const verified = receipt.status === 1;
      console.log(verified ? '✅ Verification successful!' : '❌ Verification failed');
      console.log('   📦 Block:', receipt.blockNumber);
      console.log('   ⛽ Gas used:', receipt.gasUsed.toString());

      if (verified) {
        console.log('✅ Proof verified on-chain!');
        setVerificationError(null);
        setVerifiedProofIds(prev => new Set(prev).add(selectedProof.proof_id));
      } else {
        const errorMsg = `On-chain verification failed - transaction reverted (status: ${receipt.status})`;
        console.error(errorMsg);
        setVerificationError(errorMsg);
        throw new Error(errorMsg);
      }
    } catch (error) {
      console.error('Verification error:', error);
      const errorMessage = error instanceof Error ? error.message : String(error);

      // Check if user rejected the signature
      if (
        errorMessage.includes('user rejected') ||
        errorMessage.includes('User rejected') ||
        errorMessage.includes('User denied') ||
        errorMessage.includes('4001')
      ) {
        console.log('User cancelled the signature request');
        setVerificationError(null);
      } else {
        console.error('Verification error:', errorMessage);
        setVerificationError(errorMessage);
      }
    } finally {
      setIsVerifying(false);
    }
  };

  return { handleVerify, isVerifying, isConnected, address, verifiedProofIds, verificationError, setVerificationError };
};
