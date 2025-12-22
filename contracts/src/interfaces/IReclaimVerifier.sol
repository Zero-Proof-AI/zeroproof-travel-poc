// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @title IReclaimVerifier
/// @notice Interface for Reclaim Protocol zkTLS proof verifier
/// @dev See: https://docs.reclaimprotocol.org/onchain/solidity
interface IReclaimVerifier {
    struct ClaimInfo {
        string provider;
        string parameters;
        string context;
    }

    struct CompleteClaimData {
        bytes32 identifier;
        address owner;
        uint32 timestampS;
        uint32 epoch;
    }

    struct SignedClaim {
        CompleteClaimData claim;
        bytes[] signatures;
    }

    struct Proof {
        ClaimInfo claimInfo;
        SignedClaim signedClaim;
    }

    /// @notice Verifies a Reclaim Protocol zkTLS proof
    /// @param proof The Reclaim proof containing claim data and witness signatures
    /// @return bool True if proof is valid, reverts otherwise
    function verifyProof(Proof memory proof) external returns (bool);
}
