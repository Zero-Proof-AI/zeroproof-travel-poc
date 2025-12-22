// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "@openzeppelin/contracts/access/Ownable.sol";
import "./interfaces/ISP1Verifier.sol";
import "./interfaces/IReclaimVerifier.sol";

/// @title ZeroProof
/// @notice Universal entry point for verifying multiple proof types (SP1 zkVM, Reclaim zkTLS, etc.)
/// @dev Supports extensible proof verification through a registry pattern
contract ZeroProof is Ownable {
    // ============ Types ============
    
    /// @notice Standardized claim structure for all proof types
    struct Claim {
        address agent;          // Agent that generated the claim
        bytes32 claimType;      // Type: keccak256("booking"), keccak256("pricing"), etc.
        bytes publicData;       // JSON-encoded public fields (booking_id, price, etc.)
        bytes32 dataHash;       // Hash of the claimed data for verification
    }

    // ============ Storage ============
    
    /// @notice Registry mapping proof type ID to verifier contract address
    /// @dev Key: keccak256("sp1-zkvm"), keccak256("reclaim-zktls"), etc.
    mapping(bytes32 => address) public verifiers;

    /// @notice Track verified proofs to prevent replay attacks
    mapping(bytes32 => bool) public verifiedProofs;

    // ============ Constants ============
    
    bytes32 public constant SP1_ZKVM = keccak256("sp1-zkvm");
    bytes32 public constant RECLAIM_ZKTLS = keccak256("reclaim-zktls");

    // ============ Events ============
    
    event ProofVerified(
        bytes32 indexed proofType,
        bytes32 indexed claimType,
        address indexed agent,
        bytes32 claimHash,
        uint256 timestamp
    );

    event VerifierRegistered(
        bytes32 indexed proofType,
        address indexed verifier,
        uint256 timestamp
    );

    event VerifierRemoved(
        bytes32 indexed proofType,
        uint256 timestamp
    );

    // ============ Errors ============
    
    error UnsupportedProofType(bytes32 proofType);
    error ProofAlreadyVerified(bytes32 proofHash);
    error VerificationFailed();
    error InvalidVerifierAddress();

    // ============ Constructor ============
    
    /// @notice Initialize ZeroProof with SP1 and Reclaim verifiers
    /// @param _sp1Verifier Address of deployed SP1VerifierGroth16 contract
    /// @param _reclaimVerifier Address of deployed Reclaim contract
    constructor(
        address _sp1Verifier,
        address _reclaimVerifier
    ) Ownable(msg.sender) {
        if (_sp1Verifier == address(0) || _reclaimVerifier == address(0)) {
            revert InvalidVerifierAddress();
        }
        
        verifiers[SP1_ZKVM] = _sp1Verifier;
        verifiers[RECLAIM_ZKTLS] = _reclaimVerifier;

        emit VerifierRegistered(SP1_ZKVM, _sp1Verifier, block.timestamp);
        emit VerifierRegistered(RECLAIM_ZKTLS, _reclaimVerifier, block.timestamp);
    }

    // ============ External Functions ============
    
    /// @notice Verify a proof using the appropriate verifier
    /// @param proofType Type of proof (SP1_ZKVM or RECLAIM_ZKTLS)
    /// @param proof Encoded proof data (format depends on proofType)
    /// @param claim Standardized claim structure
    /// @return verified True if proof is valid
    function verifyProof(
        bytes32 proofType,
        bytes calldata proof,
        Claim calldata claim
    ) external returns (bool verified) {
        // Check if verifier exists
        address verifier = verifiers[proofType];
        if (verifier == address(0)) {
            revert UnsupportedProofType(proofType);
        }

        // Calculate proof hash to prevent replay
        bytes32 proofHash = keccak256(abi.encode(proofType, proof, claim));
        if (verifiedProofs[proofHash]) {
            revert ProofAlreadyVerified(proofHash);
        }

        // Route to appropriate verifier
        if (proofType == SP1_ZKVM) {
            verified = _verifySP1Proof(verifier, proof, claim);
        } else if (proofType == RECLAIM_ZKTLS) {
            verified = _verifyReclaimProof(verifier, proof);
        } else {
            // For future proof types, attempt generic call
            verified = _verifyGenericProof(verifier, proof);
        }

        if (!verified) {
            revert VerificationFailed();
        }

        // Mark as verified
        verifiedProofs[proofHash] = true;

        // Calculate claim hash for event
        bytes32 claimHash = keccak256(abi.encode(claim));

        emit ProofVerified(
            proofType,
            claim.claimType,
            claim.agent,
            claimHash,
            block.timestamp
        );

        return true;
    }

    /// @notice Register a new proof type verifier
    /// @param proofType Identifier for the proof type (e.g., keccak256("noir-plonk"))
    /// @param verifier Address of the verifier contract
    function registerVerifier(
        bytes32 proofType,
        address verifier
    ) external onlyOwner {
        if (verifier == address(0)) {
            revert InvalidVerifierAddress();
        }

        verifiers[proofType] = verifier;
        emit VerifierRegistered(proofType, verifier, block.timestamp);
    }

    /// @notice Remove a proof type verifier
    /// @param proofType Identifier for the proof type to remove
    function removeVerifier(bytes32 proofType) external onlyOwner {
        delete verifiers[proofType];
        emit VerifierRemoved(proofType, block.timestamp);
    }

    /// @notice Check if a proof has been verified before
    /// @param proofType Type of proof
    /// @param proof Encoded proof data
    /// @param claim Claim structure
    /// @return True if already verified
    function isProofVerified(
        bytes32 proofType,
        bytes calldata proof,
        Claim calldata claim
    ) external view returns (bool) {
        bytes32 proofHash = keccak256(abi.encode(proofType, proof, claim));
        return verifiedProofs[proofHash];
    }

    // ============ Internal Functions ============
    
    /// @notice Verify SP1 zkVM proof
    /// @dev Expects proof format: abi.encode(programVKey, publicValues, proofBytes)
    function _verifySP1Proof(
        address verifier,
        bytes calldata proof,
        Claim calldata claim
    ) internal view returns (bool) {
        // Decode SP1 proof components
        (bytes32 programVKey, bytes memory publicValues, bytes memory proofBytes) = 
            abi.decode(proof, (bytes32, bytes, bytes));

        // Verify the public values match the claimed data hash
        bytes32 computedHash = keccak256(publicValues);
        if (computedHash != claim.dataHash) {
            return false;
        }

        // Call SP1 verifier (reverts on invalid proof)
        try ISP1Verifier(verifier).verifyProof(programVKey, publicValues, proofBytes) {
            return true;
        } catch {
            return false;
        }
    }

    /// @notice Verify Reclaim zkTLS proof
    /// @dev Expects proof format: abi.encode(IReclaimVerifier.Proof)
    function _verifyReclaimProof(
        address verifier,
        bytes calldata proof
    ) internal returns (bool) {
        // Decode Reclaim proof
        IReclaimVerifier.Proof memory reclaimProof = 
            abi.decode(proof, (IReclaimVerifier.Proof));

        // Call Reclaim verifier
        try IReclaimVerifier(verifier).verifyProof(reclaimProof) returns (bool valid) {
            return valid;
        } catch {
            return false;
        }
    }

    /// @notice Generic proof verification for future proof types
    /// @dev Attempts to call verifyProof(bytes) on the verifier
    function _verifyGenericProof(
        address verifier,
        bytes calldata proof
    ) internal returns (bool) {
        // Low-level call to verifyProof(bytes)
        (bool success, bytes memory result) = verifier.call(
            abi.encodeWithSignature("verifyProof(bytes)", proof)
        );

        if (!success) {
            return false;
        }

        // Decode return value
        if (result.length > 0) {
            return abi.decode(result, (bool));
        }

        return false;
    }
}
