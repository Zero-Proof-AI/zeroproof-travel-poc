import React from 'react';

interface CryptographicProof {
  tool_name: string;
  timestamp: number;
  proof_id?: string;
  verified: boolean;
  onchain_compatible: boolean;
  sequence?: number;
  related_proof_id?: string;
  workflow_stage?: string;
  submitted_by?: string;
}

interface ProofBadgeProps {
  proof: CryptographicProof;
  index: number;
  expandedProofIds: Set<string>;
  onToggleExpanded: (proofKey: string) => void;
  onFetchFullProof: (proofId: string) => void;
  proofModalLoading: boolean;
}

const ProofBadge = React.memo(
  ({
    proof,
    index,
    expandedProofIds,
    onToggleExpanded,
    onFetchFullProof,
    proofModalLoading,
  }: ProofBadgeProps) => {
    const proofKey = proof.proof_id || `${proof.tool_name}-${index}`;
    const isExpanded = expandedProofIds.has(proofKey);

    const toggleExpanded = () => {
      onToggleExpanded(proofKey);
    };

    const workflowColors: { [key: string]: string } = {
      pricing: '#e6f3ff',
      payment_enrollment: '#fff0f5',
      payment: '#fff5e6',
      booking: '#e6ffe6',
    };
    const workflowBorders: { [key: string]: string } = {
      pricing: '#4299e1',
      payment_enrollment: '#ed64a6',
      payment: '#f6ad55',
      booking: '#48bb78',
    };

    const stageColor = workflowColors[proof.workflow_stage || 'unknown'] || '#f0f4f8';
    const stageBorder = workflowBorders[proof.workflow_stage || 'unknown'] || '#cbd5e0';

    return (
      <div
        onClick={toggleExpanded}
        style={{
          marginTop: '0.5rem',
          padding: '0.75rem',
          background: stageColor,
          border: `2px solid ${stageBorder}`,
          borderRadius: '0.5rem',
          fontSize: '0.85rem',
          borderLeft: `4px solid ${proof.verified ? '#48bb78' : '#f56565'}`,
          cursor: 'pointer',
          transition: 'all 0.2s ease',
          boxShadow: isExpanded ? '0 4px 8px rgba(0,0,0,0.1)' : 'none',
        }}
      >
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'start' }}>
          <div style={{ flex: 1 }}>
            <div
              style={{
                fontWeight: 'bold',
                color: proof.verified ? '#22543d' : '#742a2a',
                fontSize: '0.95rem',
                display: 'flex',
                alignItems: 'center',
                gap: '0.5rem',
              }}
            >
              <span
                style={{
                  fontSize: '1rem',
                  transition: 'transform 0.2s ease',
                  transform: isExpanded ? 'rotate(90deg)' : 'rotate(0deg)',
                }}
              >
                ▶
              </span>
              {proof.sequence && (
                <span
                  style={{
                    marginRight: '0.5rem',
                    background: stageBorder,
                    color: '#fff',
                    padding: '0.2rem 0.4rem',
                    borderRadius: '0.25rem',
                    fontSize: '0.75rem',
                  }}
                >
                  #{proof.sequence}
                </span>
              )}
              🔐 {proof.verified ? '✓ Verified' : '✗ Unverified'} Proof
            </div>
            <div style={{ marginTop: '0.25rem', color: '#4a5568' }}>
              <strong>Tool:</strong> {proof.tool_name}
            </div>
            {proof.workflow_stage && (
              <div style={{ marginTop: '0.25rem', color: '#4a5568' }}>
                <strong>Stage:</strong>{' '}
                <span
                  style={{
                    textTransform: 'uppercase',
                    fontSize: '0.75rem',
                    background: stageBorder,
                    color: '#fff',
                    padding: '0.1rem 0.3rem',
                    borderRadius: '0.2rem',
                  }}
                >
                  {proof.workflow_stage}
                </span>
              </div>
            )}
            {proof.submitted_by && (
              <div style={{ marginTop: '0.25rem', color: '#4a5568' }}>
                <strong>Submitted By:</strong> {proof.submitted_by}
              </div>
            )}
            {!isExpanded && proof.proof_id && (
              <div
                onClick={(e) => {
                  e.stopPropagation();
                  onFetchFullProof(proof.proof_id!);
                }}
                style={{
                  marginTop: '0.25rem',
                  color: '#2563eb',
                  fontSize: '0.75rem',
                  wordBreak: 'break-all',
                  fontFamily: 'monospace',
                  background: 'rgba(37, 99, 235, 0.05)',
                  padding: '0.25rem',
                  borderRadius: '0.2rem',
                  cursor: 'pointer',
                  textDecoration: 'underline',
                  transition: 'all 0.2s ease',
                }}
                onMouseEnter={(e) => (e.currentTarget.style.background = 'rgba(37, 99, 235, 0.1)')}
                onMouseLeave={(e) => (e.currentTarget.style.background = 'rgba(37, 99, 235, 0.05)')}
              >
                <strong>ID:</strong> {proof.proof_id.substring(0, 32)}...{' '}
                <span style={{ fontSize: '0.65rem' }}>🔍 click to verify</span>
              </div>
            )}
            <div style={{ marginTop: '0.25rem', color: '#4a5568' }}>
              <strong>On-chain:</strong> {proof.onchain_compatible ? '✓ Yes' : '✗ No'}
            </div>
          </div>
        </div>

        {proof.related_proof_id && (
          <div
            style={{
              marginTop: '0.25rem',
              padding: '0.25rem',
              background: 'rgba(0,0,0,0.02)',
              borderRadius: '0.2rem',
              fontSize: '0.75rem',
              color: '#4a5568',
            }}
          >
            ↳ Related to: {proof.related_proof_id.substring(0, 16)}...
          </div>
        )}

        {/* Expanded Details */}
        {isExpanded && (
          <div
            style={{
              marginTop: '0.75rem',
              paddingTop: '0.75rem',
              borderTop: `1px solid ${stageBorder}`,
              animation: 'fadeIn 0.2s ease',
            }}
          >
            <div
              style={{
                background: 'rgba(0,0,0,0.02)',
                padding: '0.75rem',
                borderRadius: '0.35rem',
                fontSize: '0.8rem',
                fontFamily: 'monospace',
                overflowX: 'auto',
              }}
            >
              <div style={{ marginBottom: '0.5rem' }}>
                <strong style={{ color: '#2d3748' }}>Proof ID:</strong>
                <div
                  style={{
                    marginTop: '0.25rem',
                    color: '#4a5568',
                    wordBreak: 'break-all',
                    background: '#fff',
                    padding: '0.35rem',
                    borderRadius: '0.25rem',
                    display: 'flex',
                    justifyContent: 'space-between',
                    alignItems: 'center',
                  }}
                >
                  <span>{proof.proof_id || 'Not available'}</span>
                  {proof.proof_id && (
                    <button
                      onClick={(e) => {
                        e.stopPropagation();
                        onFetchFullProof(proof.proof_id!);
                      }}
                      style={{
                        marginLeft: '0.5rem',
                        padding: '0.25rem 0.5rem',
                        background: '#2563eb',
                        color: '#fff',
                        border: 'none',
                        borderRadius: '0.2rem',
                        fontSize: '0.7rem',
                        cursor: 'pointer',
                        whiteSpace: 'nowrap',
                      }}
                      disabled={proofModalLoading}
                    >
                      {proofModalLoading ? '🔄 Loading...' : '🔍 View Full'}
                    </button>
                  )}
                </div>
              </div>

              <div style={{ marginBottom: '0.5rem' }}>
                <strong style={{ color: '#2d3748' }}>Timestamp:</strong>
                <div
                  style={{
                    marginTop: '0.25rem',
                    color: '#4a5568',
                    background: '#fff',
                    padding: '0.35rem',
                    borderRadius: '0.25rem',
                  }}
                >
                  {new Date(proof.timestamp * 1000).toLocaleString()}
                </div>
              </div>

              <div style={{ marginBottom: '0.5rem' }}>
                <strong style={{ color: '#2d3748' }}>Verified:</strong>
                <div
                  style={{
                    marginTop: '0.25rem',
                    color: proof.verified ? '#22543d' : '#742a2a',
                    background: '#fff',
                    padding: '0.35rem',
                    borderRadius: '0.25rem',
                  }}
                >
                  {proof.verified
                    ? '✓ Yes (Cryptographically signed by Reclaim)'
                    : '✗ No (Proof validation pending)'}
                </div>
              </div>

              <div style={{ marginBottom: '0.5rem' }}>
                <strong style={{ color: '#2d3748' }}>On-Chain Compatible:</strong>
                <div
                  style={{
                    marginTop: '0.25rem',
                    color: '#4a5568',
                    background: '#fff',
                    padding: '0.35rem',
                    borderRadius: '0.25rem',
                  }}
                >
                  {proof.onchain_compatible
                    ? '✓ Yes (Can be submitted to blockchain)'
                    : '✗ No (Requires additional processing)'}
                </div>
              </div>

              {proof.workflow_stage && (
                <div style={{ marginBottom: '0.5rem' }}>
                  <strong style={{ color: '#2d3748' }}>Workflow Stage:</strong>
                  <div
                    style={{
                      marginTop: '0.25rem',
                      color: '#4a5568',
                      background: '#fff',
                      padding: '0.35rem',
                      borderRadius: '0.25rem',
                      textTransform: 'capitalize',
                    }}
                  >
                    {proof.workflow_stage}
                  </div>
                </div>
              )}

              {proof.submitted_by && (
                <div style={{ marginBottom: '0.5rem' }}>
                  <strong style={{ color: '#2d3748' }}>Submitted By:</strong>
                  <div
                    style={{
                      marginTop: '0.25rem',
                      color: '#4a5568',
                      background: '#fff',
                      padding: '0.35rem',
                      borderRadius: '0.25rem',
                    }}
                  >
                    {proof.submitted_by}
                  </div>
                </div>
              )}

              {proof.sequence && (
                <div style={{ marginBottom: '0.5rem' }}>
                  <strong style={{ color: '#2d3748' }}>Sequence Number:</strong>
                  <div
                    style={{
                      marginTop: '0.25rem',
                      color: '#4a5568',
                      background: '#fff',
                      padding: '0.35rem',
                      borderRadius: '0.25rem',
                    }}
                  >
                    {proof.sequence}
                  </div>
                </div>
              )}

              {proof.related_proof_id && (
                <div style={{ marginBottom: '0.5rem' }}>
                  <strong style={{ color: '#2d3748' }}>Related Proof ID:</strong>
                  <div
                    style={{
                      marginTop: '0.25rem',
                      color: '#4a5568',
                      background: '#fff',
                      padding: '0.35rem',
                      borderRadius: '0.25rem',
                      wordBreak: 'break-all',
                    }}
                  >
                    {proof.related_proof_id}
                  </div>
                </div>
              )}

              <div
                style={{
                  marginTop: '0.75rem',
                  padding: '0.5rem',
                  background: '#e6f0ff',
                  borderRadius: '0.25rem',
                  fontSize: '0.75rem',
                  color: '#2c5282',
                  lineHeight: '1.4',
                }}
              >
                <strong>What this proves:</strong>
                <div style={{ marginTop: '0.25rem' }}>
                  ✓ Agent-A made an authenticated HTTPS request to the {proof.tool_name} endpoint
                  <br />
                  ✓ The response data is genuine and cryptographically verified (Zero-Knowledge TLS)
                  <br />
                  ✓ No intermediary could have tampered with the data
                  <br />
                  {proof.onchain_compatible &&
                    '✓ This proof can be stored permanently on blockchain for audit trail'}
                </div>
              </div>
            </div>
          </div>
        )}
      </div>
    );
  },
  (prevProps, nextProps) => {
    // Return true if props haven't changed (skip re-render)
    // Return false if props have changed (do re-render)
    return (
      JSON.stringify(prevProps.proof) === JSON.stringify(nextProps.proof) &&
      prevProps.index === nextProps.index &&
      prevProps.expandedProofIds === nextProps.expandedProofIds &&
      prevProps.proofModalLoading === nextProps.proofModalLoading
    );
  }
);

ProofBadge.displayName = 'ProofBadge';

export default ProofBadge;
