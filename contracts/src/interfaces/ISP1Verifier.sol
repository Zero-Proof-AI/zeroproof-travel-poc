// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @title ISP1Verifier
/// @notice Interface for SP1 Groth16 universal verifier
/// @dev See: https://docs.succinct.xyz/verification/onchain/solidity-sdk.html
interface ISP1Verifier {
    /// @notice Verifies a SP1 RISC-V zkVM proof
    /// @param programVKey The verification key hash (32 bytes) for the RISC-V program
    /// @param publicValues The public outputs from the zkVM execution
    /// @param proofBytes The Groth16 proof (8 * 32 = 256 bytes)
    /// @dev Reverts with InvalidProof() if verification fails
    /// @dev Reverts with WrongVerifierSelector() if programVKey doesn't match proof's VERIFIER_HASH
    function verifyProof(
        bytes32 programVKey,
        bytes calldata publicValues,
        bytes calldata proofBytes
    ) external view;
}
