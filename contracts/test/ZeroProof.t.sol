// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/ZeroProof.sol";

contract MockSP1Verifier {
    function verifyProof(
        bytes32 programVKey,
        bytes calldata publicValues,
        bytes calldata proofBytes
    ) external pure {
        // Mock: Always succeeds (in real tests, check actual proof)
        require(programVKey != bytes32(0), "Invalid vkey");
        require(publicValues.length > 0, "Empty public values");
        require(proofBytes.length == 256, "Invalid proof length");
    }
}

contract MockReclaimVerifier {
    function verifyProof(
        IReclaimVerifier.Proof memory proof
    ) external pure returns (bool) {
        // Mock: Always returns true if provider is set
        return bytes(proof.claimInfo.provider).length > 0;
    }
}

contract ZeroProofTest is Test {
    ZeroProof public zeroProof;
    MockSP1Verifier public mockSP1;
    MockReclaimVerifier public mockReclaim;

    address public alice = address(0x1);
    address public bob = address(0x2);

    function setUp() public {
        mockSP1 = new MockSP1Verifier();
        mockReclaim = new MockReclaimVerifier();
        zeroProof = new ZeroProof(address(mockSP1), address(mockReclaim));
    }

    function testConstructor() public view {
        assertEq(zeroProof.verifiers(zeroProof.SP1_ZKVM()), address(mockSP1));
        assertEq(zeroProof.verifiers(zeroProof.RECLAIM_ZKTLS()), address(mockReclaim));
    }

    function testVerifySP1Proof() public {
        // Create mock SP1 proof
        bytes32 vkey = keccak256("test-program");
        bytes memory publicValues = abi.encode(uint256(578)); // price = 578
        bytes memory proofBytes = new bytes(256); // Mock 256-byte proof
        
        bytes memory encodedProof = abi.encode(vkey, publicValues, proofBytes);

        ZeroProof.Claim memory claim = ZeroProof.Claim({
            agent: bob,
            claimType: keccak256("pricing"),
            publicData: abi.encode("NYC", "LON", true, 578),
            dataHash: keccak256(publicValues)
        });

        // Verify proof
        vm.prank(alice);
        bool verified = zeroProof.verifyProof(
            zeroProof.SP1_ZKVM(),
            encodedProof,
            claim
        );

        assertTrue(verified);
    }

    function testVerifyReclaimProof() public {
        // Create mock Reclaim proof
        IReclaimVerifier.Proof memory proof = IReclaimVerifier.Proof({
            claimInfo: IReclaimVerifier.ClaimInfo({
                provider: "http",
                parameters: '{"url":"https://api.aa.com/book"}',
                context: '{"booking_id":"ABC123"}'
            }),
            signedClaim: IReclaimVerifier.SignedClaim({
                claim: IReclaimVerifier.CompleteClaimData({
                    identifier: keccak256("test"),
                    owner: bob,
                    timestampS: uint32(block.timestamp),
                    epoch: 1
                }),
                signatures: new bytes[](1)
            })
        });

        bytes memory encodedProof = abi.encode(proof);

        ZeroProof.Claim memory claim = ZeroProof.Claim({
            agent: bob,
            claimType: keccak256("booking"),
            publicData: '{"booking_id":"ABC123"}',
            dataHash: keccak256("ABC123")
        });

        // Verify proof
        vm.prank(alice);
        bool verified = zeroProof.verifyProof(
            zeroProof.RECLAIM_ZKTLS(),
            encodedProof,
            claim
        );

        assertTrue(verified);
    }

    function testRejectReplayAttack() public {
        bytes32 vkey = keccak256("test-program");
        bytes memory publicValues = abi.encode(uint256(578));
        bytes memory proofBytes = new bytes(256);
        bytes memory encodedProof = abi.encode(vkey, publicValues, proofBytes);

        ZeroProof.Claim memory claim = ZeroProof.Claim({
            agent: bob,
            claimType: keccak256("pricing"),
            publicData: abi.encode("NYC", "LON", true, 578),
            dataHash: keccak256(publicValues)
        });

        // First verification succeeds
        zeroProof.verifyProof(zeroProof.SP1_ZKVM(), encodedProof, claim);

        // Second verification fails (replay)
        vm.expectRevert();
        zeroProof.verifyProof(zeroProof.SP1_ZKVM(), encodedProof, claim);
    }

    function testRejectUnsupportedProofType() public {
        bytes32 unknownType = keccak256("unknown-proof-type");
        
        ZeroProof.Claim memory claim = ZeroProof.Claim({
            agent: bob,
            claimType: keccak256("pricing"),
            publicData: "",
            dataHash: bytes32(0)
        });

        vm.expectRevert(abi.encodeWithSelector(ZeroProof.UnsupportedProofType.selector, unknownType));
        zeroProof.verifyProof(unknownType, "", claim);
    }

    function testRegisterNewVerifier() public {
        bytes32 noirType = keccak256("noir-plonk");
        address noirVerifier = address(0x999);

        zeroProof.registerVerifier(noirType, noirVerifier);

        assertEq(zeroProof.verifiers(noirType), noirVerifier);
    }

    function testOnlyOwnerCanRegisterVerifier() public {
        bytes32 noirType = keccak256("noir-plonk");
        address noirVerifier = address(0x999);

        vm.prank(alice);
        vm.expectRevert();
        zeroProof.registerVerifier(noirType, noirVerifier);
    }
}
