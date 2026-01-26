import React from 'react';
import { useProofVerification } from './useProofVerification';

export interface FullProofData {
  proof_id: string;
  session_id: string;
  tool_name: string;
  timestamp: number;
  request: any;
  response: any;
  proof: any;
  verified: boolean;
  onchain_compatible: boolean;
  submitted_by?: string;
  sequence?: number;
  related_proof_id?: string;
  workflow_stage?: string;
  verification_info?: {
    protocol: string;
    issuer: string;
    timestamp_verified: boolean;
    signature_algorithm: string;
    can_verify_onchain: boolean;
    reclaim_documentation: string;
  };
}

interface ProofModalProps {
  open: boolean;
  selectedProof: FullProofData | null;
  onClose: () => void;
}

const ProofModal: React.FC<ProofModalProps> = React.memo(({ open, selectedProof, onClose }) => {
  const { handleVerify, isVerifying, isConnected, verifiedProofIds, verificationError, setVerificationError } = useProofVerification();
  const isVerified = selectedProof ? verifiedProofIds.has(selectedProof.proof_id) : false;

  if (!open || !selectedProof) return null;
  
  return (
    <div
      style={{
        position: 'fixed',
        top: 0,
        left: 0,
        right: 0,
        bottom: 0,
        background: 'rgba(0,0,0,0.5)',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        zIndex: 1000,
      }}
      onClick={onClose}
    >
      <div
        style={{
          background: '#fff',
          borderRadius: '0.5rem',
          maxWidth: '90vw',
          maxHeight: '90vh',
          overflow: 'auto',
          padding: '2rem',
          boxShadow: '0 20px 60px rgba(0,0,0,0.3)',
          animation: 'slideUp 0.3s ease',
        }}
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'start', marginBottom: '1.5rem' }}>
          <div>
            <h2 style={{ margin: 0, color: '#2d3748', fontSize: '1.5rem' }}>
              🔐 Full Proof Verification
            </h2>
            <p style={{ margin: '0.5rem 0 0 0', color: '#718096', fontSize: '0.9rem' }}>
              Complete on-chain verifiable proof data
            </p>
          </div>
          <button
            onClick={onClose}
            style={{
              background: 'none',
              border: 'none',
              fontSize: '1.5rem',
              cursor: 'pointer',
              color: '#718096',
            }}
          >
            ✕
          </button>
        </div>

        {/* Core Information */}
        <div style={{ marginBottom: '1.5rem', paddingBottom: '1rem', borderBottom: '1px solid #e2e8f0' }}>
          <h3 style={{ color: '#2d3748', marginBottom: '0.75rem' }}>Core Information</h3>
          <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: '1rem' }}>
            <div>
              <strong style={{ color: '#4a5568' }}>Proof ID:</strong>
              <div style={{ marginTop: '0.25rem', fontSize: '0.85rem', fontFamily: 'monospace', wordBreak: 'break-all', background: '#f7fafc', padding: '0.5rem', borderRadius: '0.25rem' }}>
                {selectedProof.proof_id}
              </div>
            </div>
            <div>
              <strong style={{ color: '#4a5568' }}>Session ID:</strong>
              <div style={{ marginTop: '0.25rem', fontSize: '0.85rem', fontFamily: 'monospace', wordBreak: 'break-all', background: '#f7fafc', padding: '0.5rem', borderRadius: '0.25rem' }}>
                {selectedProof.session_id}
              </div>
            </div>
            <div>
              <strong style={{ color: '#4a5568' }}>Tool:</strong>
              <div style={{ marginTop: '0.25rem', fontSize: '0.85rem', background: '#f7fafc', padding: '0.5rem', borderRadius: '0.25rem' }}>
                {selectedProof.tool_name}
              </div>
            </div>
            <div>
              <strong style={{ color: '#4a5568' }}>Timestamp:</strong>
              <div style={{ marginTop: '0.25rem', fontSize: '0.85rem', background: '#f7fafc', padding: '0.5rem', borderRadius: '0.25rem' }}>
                {new Date(selectedProof.timestamp * 1000).toLocaleString()}
              </div>
            </div>
          </div>
        </div>

        {/* Verification Status */}
        <div style={{ marginBottom: '1.5rem', paddingBottom: '1rem', borderBottom: '1px solid #e2e8f0' }}>
          <h3 style={{ color: '#2d3748', marginBottom: '0.75rem' }}>Verification Status</h3>
          <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: '1rem' }}>
            <div style={{ background: selectedProof.verified ? '#ecfdf5' : '#fef2f2', padding: '0.75rem', borderRadius: '0.25rem', borderLeft: `3px solid ${selectedProof.verified ? '#10b981' : '#ef4444'}` }}>
              <strong style={{ color: selectedProof.verified ? '#065f46' : '#7f1d1d' }}>
                {selectedProof.verified ? '✓ Verified' : '✗ Unverified'}
              </strong>
              <p style={{ margin: '0.25rem 0 0 0', fontSize: '0.85rem', color: selectedProof.verified ? '#047857' : '#991b1b' }}>
                {selectedProof.verified ? 'Cryptographically signed by Reclaim' : 'Verification pending'}
              </p>
            </div>
            <div style={{ background: selectedProof.onchain_compatible ? '#ecfdf5' : '#fef2f2', padding: '0.75rem', borderRadius: '0.25rem', borderLeft: `3px solid ${selectedProof.onchain_compatible ? '#10b981' : '#ef4444'}` }}>
              <strong style={{ color: selectedProof.onchain_compatible ? '#065f46' : '#7f1d1d' }}>
                {selectedProof.onchain_compatible ? '✓ On-Chain Ready' : '✗ On-Chain Processing'}
              </strong>
              <p style={{ margin: '0.25rem 0 0 0', fontSize: '0.85rem', color: selectedProof.onchain_compatible ? '#047857' : '#991b1b' }}>
                {selectedProof.onchain_compatible ? 'Ready to submit to blockchain' : 'Converting for on-chain compatibility'}
              </p>
            </div>
          </div>
        </div>

        {/* Request/Response */}
        <div style={{ marginBottom: '1.5rem', paddingBottom: '1rem', borderBottom: '1px solid #e2e8f0' }}>
          <h3 style={{ color: '#2d3748', marginBottom: '0.75rem' }}>Request & Response</h3>
          <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: '1rem' }}>
            <div>
              <strong style={{ color: '#4a5568', fontSize: '0.85rem' }}>Request:</strong>
              <pre style={{ background: '#f7fafc', padding: '0.75rem', borderRadius: '0.25rem', fontSize: '0.75rem', overflow: 'auto', maxHeight: '200px', marginTop: '0.5rem' }}>
                {JSON.stringify(selectedProof.request, null, 2)}
              </pre>
            </div>
            <div>
              <strong style={{ color: '#4a5568', fontSize: '0.85rem' }}>Response:</strong>
              <pre style={{ background: '#f7fafc', padding: '0.75rem', borderRadius: '0.25rem', fontSize: '0.75rem', overflow: 'auto', maxHeight: '200px', marginTop: '0.5rem' }}>
                {JSON.stringify(selectedProof.response, null, 2)}
              </pre>
            </div>
          </div>
        </div>

        {/* ZK-TLS Proof */}
        <div style={{ marginBottom: '1.5rem', paddingBottom: '1rem', borderBottom: '1px solid #e2e8f0' }}>
          <h3 style={{ color: '#2d3748', marginBottom: '0.75rem', display: 'flex', alignItems: 'center', gap: '0.5rem' }}>
            ZK-TLS Proof 
            <button
              onClick={() => window.open('https://sepolia.etherscan.io/address/0x9C33252D29B41Fe2706704a8Ca99E8731B58af41#code', '_blank')}
              style={{ 
                background: '#2563eb', 
                color: '#fff', 
                border: 'none', 
                borderRadius: '0.25rem', 
                padding: '0.25rem 0.5rem',
                fontSize: '0.8em', 
                cursor: 'pointer',
                fontWeight: '500'
              }}
              // onMouseEnter={(e) => { e.currentTarget.style.background = '#1d4ed8'; }}
              // onMouseLeave={(e) => { e.currentTarget.style.background = '#2563eb'; }}
            >
              See contract
            </button>
          </h3>
          <details style={{ cursor: 'pointer' }}>
            <summary style={{ padding: '0.5rem', background: '#f7fafc', borderRadius: '0.25rem', userSelect: 'none' }}>
              <strong>Click to expand proof data</strong> (for on-chain verification)
            </summary>
            <button
              onClick={() => {
                setVerificationError(null);
                handleVerify(selectedProof);
              }}
              style={{ 
                marginTop: '0.5rem',
                background: isVerified
                  ? '#10b981'
                  : verificationError
                  ? '#ef4444'
                  : isVerifying
                  ? '#6b7280'
                  : '#3b82f6', 
                color: '#fff', 
                border: 'none', 
                borderRadius: '0.25rem', 
                padding: '0.5rem 1rem',
                fontSize: '0.9em', 
                fontWeight: '500',
                cursor: (isVerified || isVerifying) && !verificationError ? 'not-allowed' : 'pointer',
                opacity: (isVerified || isVerifying) && !verificationError ? 0.7 : 1,
                transition: 'all 0.2s ease',
              }}
              onMouseEnter={(e) => {
                if (!isVerifying && !isVerified && !verificationError) {
                  e.currentTarget.style.opacity = '0.9';
                  e.currentTarget.style.transform = 'translateY(-2px)';
                }
              }}
              onMouseLeave={(e) => {
                if (!isVerifying && !isVerified && !verificationError) {
                  e.currentTarget.style.opacity = '1';
                  e.currentTarget.style.transform = 'translateY(0)';
                }
              }}
              disabled={isVerifying || (isVerified && !verificationError)}
            >
              {isVerified && !verificationError ? '✅ Verified' : verificationError ? '❌ Failed - Try Again' : isVerifying ? '🔄 Verifying...' : 'Verify'}
            </button>
            {verificationError && (
              <div style={{ 
                marginTop: '0.75rem', 
                padding: '0.75rem', 
                background: '#fee2e2', 
                border: '1px solid #fecaca', 
                borderRadius: '0.25rem', 
                color: '#991b1b',
                fontSize: '0.85rem'
              }}>
                <strong style={{ display: 'block', marginBottom: '0.25rem' }}>❌ Verification Failed</strong>
                <p style={{ margin: 0 }}>{verificationError}</p>
              </div>
            )}
            <pre style={{ background: '#f7fafc', padding: '0.75rem', borderRadius: '0.25rem', fontSize: '0.75rem', overflow: 'auto', maxHeight: '300px', marginTop: '0.5rem' }}>
              {JSON.stringify(selectedProof.proof?.onchainProof || selectedProof.proof, null, 2)}
            </pre>
          </details>
        </div>

        {/* Verification Info */}
        {selectedProof.verification_info && (
          <div style={{ marginBottom: '1.5rem', paddingBottom: '1rem', borderBottom: '1px solid #e2e8f0' }}>
            <h3 style={{ color: '#2d3748', marginBottom: '0.75rem' }}>Verification Information</h3>
            <div style={{ background: '#f0f9ff', padding: '1rem', borderRadius: '0.35rem', borderLeft: '3px solid #0284c7' }}>
              <div style={{ marginBottom: '0.5rem' }}>
                <strong style={{ color: '#0c4a6e' }}>Protocol:</strong> {selectedProof.verification_info.protocol}
              </div>
              <div style={{ marginBottom: '0.5rem' }}>
                <strong style={{ color: '#0c4a6e' }}>Issuer:</strong> {selectedProof.verification_info.issuer}
              </div>
              <div style={{ marginBottom: '0.5rem' }}>
                <strong style={{ color: '#0c4a6e' }}>Algorithm:</strong> {selectedProof.verification_info.signature_algorithm}
              </div>
              <div>
                <strong style={{ color: '#0c4a6e' }}>Documentation:</strong> <a href={selectedProof.verification_info.reclaim_documentation} target="_blank" rel="noopener noreferrer" style={{ color: '#0284c7', textDecoration: 'underline' }}>
                  {selectedProof.verification_info.reclaim_documentation}
                </a>
              </div>
            </div>
          </div>
        )}

        {/* Metadata */}
        <div style={{ marginBottom: '1.5rem' }}>
          <h3 style={{ color: '#2d3748', marginBottom: '0.75rem' }}>Workflow Metadata</h3>
          <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: '1rem' }}>
            {selectedProof.workflow_stage && (
              <div>
                <strong style={{ color: '#4a5568', fontSize: '0.85rem' }}>Workflow Stage:</strong>
                <div style={{ marginTop: '0.25rem', background: '#f7fafc', padding: '0.35rem 0.5rem', borderRadius: '0.25rem', textTransform: 'capitalize', fontSize: '0.85rem' }}>
                  {selectedProof.workflow_stage}
                </div>
              </div>
            )}
            {selectedProof.sequence && (
              <div>
                <strong style={{ color: '#4a5568', fontSize: '0.85rem' }}>Sequence:</strong>
                <div style={{ marginTop: '0.25rem', background: '#f7fafc', padding: '0.35rem 0.5rem', borderRadius: '0.25rem', fontSize: '0.85rem' }}>
                  #{selectedProof.sequence}
                </div>
              </div>
            )}
            {selectedProof.submitted_by && (
              <div>
                <strong style={{ color: '#4a5568', fontSize: '0.85rem' }}>Submitted By:</strong>
                <div style={{ marginTop: '0.25rem', background: '#f7fafc', padding: '0.35rem 0.5rem', borderRadius: '0.25rem', fontSize: '0.85rem' }}>
                  {selectedProof.submitted_by}
                </div>
              </div>
            )}
            {selectedProof.related_proof_id && (
              <div>
                <strong style={{ color: '#4a5568', fontSize: '0.85rem' }}>Related Proof:</strong>
                <div style={{ marginTop: '0.25rem', background: '#f7fafc', padding: '0.35rem 0.5rem', borderRadius: '0.25rem', fontSize: '0.75rem', fontFamily: 'monospace', wordBreak: 'break-all' }}>
                  {selectedProof.related_proof_id}
                </div>
              </div>
            )}
          </div>
        </div>

        {/* Close Button */}
        <button
          onClick={onClose}
          style={{
            width: '100%',
            padding: '0.75rem',
            background: '#2d3748',
            color: '#fff',
            border: 'none',
            borderRadius: '0.25rem',
            cursor: 'pointer',
            fontSize: '1rem',
            fontWeight: 'bold',
          }}
        >
          Close
        </button>
      </div>
    </div>
  );
});

ProofModal.displayName = 'ProofModal';

export default ProofModal;
