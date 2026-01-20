import React, { useState, useCallback } from 'react';
import axios from 'axios';
import './App.css';

interface CryptographicProof {
  tool_name: string;
  timestamp: number;
  proof_id?: string;
  verified: boolean;
  onchain_compatible: boolean;
  sequence?: number;
  related_proof_id?: string;
  workflow_stage?: string;
}

interface FullProofData {
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

interface ChatMessage {
  role: string;
  content: string;
}

const ChatInterface: React.FC = () => {
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [input, setInput] = useState('');
  const [loading, setLoading] = useState(false);
  const [sessionId, setSessionId] = useState<string>('');
  const [showProofs, setShowProofs] = useState(false);
  const [proofs, setProofs] = useState<CryptographicProof[]>([]);
  const [proofLoading, setProofLoading] = useState(false);
  const [expandedProofIds, setExpandedProofIds] = useState<Set<string>>(new Set());
  const [selectedProof, setSelectedProof] = useState<FullProofData | null>(null);
  const [proofModalOpen, setProofModalOpen] = useState(false);
  const [proofModalLoading, setProofModalLoading] = useState(false);
  const [socket, setSocket] = useState<WebSocket | null>(null);
  const [wsConnected, setWsConnected] = useState(false);
  const messagesEndRef = React.useRef<HTMLDivElement>(null);

  // Backend URLs - point to Agent A HTTP/WebSocket Server
  // For local: http://localhost:3001, ws://localhost:3001
  // For production: https://dev.agenta.zeroproofai.com, wss://dev.agenta.zeroproofai.com
  const baseUrl = 'https://dev.agenta.zeroproofai.com';
  const wsUrl = baseUrl.replace('https://', 'wss://').replace('http://', 'ws://');
  const proofsApiUrl = `${baseUrl}/proofs`;
  const verifyProofUrl = `${baseUrl}/proofs/verify`;

  // Auto-scroll to bottom when messages change
  const scrollToBottom = () => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  };

  React.useEffect(() => {
    scrollToBottom();
  }, [messages, loading]);

  // WebSocket connection management
  const connectWebSocket = () => {
    if (socket && socket.readyState === WebSocket.OPEN) {
      return; // Already connected
    }

    console.log('[WEBSOCKET] Connecting to', wsUrl + '/ws/chat');
    const ws = new WebSocket(`${wsUrl}/ws/chat`);

    ws.onopen = () => {
      console.log('[WEBSOCKET] Connected');
      setWsConnected(true);
      setSocket(ws);
    };

    ws.onmessage = (event) => {
      try {
        const data = JSON.parse(event.data);
        console.log('[WEBSOCKET] Received:', data);

        // Store server-returned session ID for subsequent messages
        if (data.session_id && data.session_id !== sessionId) {
          console.log('[WEBSOCKET] Updating session ID from server:', data.session_id);
          setSessionId(data.session_id);
        }

        // Handle progress messages (real-time updates)
        if (data.progress) {
          console.log('[WEBSOCKET] Progress update:', data.progress);
          const progressMessage: ChatMessage = {
            role: 'assistant',
            content: data.progress,
          };
          setMessages((prev) => [...prev, progressMessage]);
        }
        // Handle final response messages
        else if (data.success && data.response) {
          const assistantMessage: ChatMessage = {
            role: 'assistant',
            content: data.response,
          };
          setMessages((prev) => [...prev, assistantMessage]);
          setLoading(false);
          
          // Fetch proofs after agent responds
          setTimeout(() => {
            if (sessionId) {
              fetchProofsAutomatically();
            }
          }, 500);
        } else if (data.error) {
          console.error('[WEBSOCKET] Error from server:', data.error);
          const errorMessage: ChatMessage = {
            role: 'assistant',
            content: `Error: ${data.error || 'Unknown error'}`,
          };
          setMessages((prev) => [...prev, errorMessage]);
          setLoading(false);
        }
      } catch (error) {
        console.error('[WEBSOCKET] Failed to parse message:', error);
      }
    };

    ws.onclose = () => {
      console.log('[WEBSOCKET] Disconnected');
      setWsConnected(false);
      setSocket(null);
    };

    ws.onerror = (error) => {
      console.error('[WEBSOCKET] Error:', error);
      setWsConnected(false);
    };
  };

  const disconnectWebSocket = () => {
    if (socket) {
      socket.close();
      setSocket(null);
      setWsConnected(false);
    }
  };

  // Initialize WebSocket connection and session on component mount
  React.useEffect(() => {
    // Session ID will be generated by server on first message
    // No need to initialize client-side session ID

    // Connect to WebSocket when component mounts
    connectWebSocket();

    // Cleanup on unmount
    return () => {
      disconnectWebSocket();
    };
  }, []);

  // Fetch proofs automatically (for polling) - no loading state to avoid blinking
  const fetchProofsAutomatically = useCallback(async () => {
    if (!sessionId) return;
    
    try {
      const response = await axios.get(`${proofsApiUrl}/${sessionId}`);
      if (response.data.success) {
        // Only update if proofs actually changed
        setProofs((prevProofs) => {
          const newProofs = response.data.proofs;
          
          // Quick check: if count is the same, likely no changes
          if (prevProofs.length === newProofs.length && prevProofs.length > 0) {
            // For same-length arrays, compare only the first and last items to detect changes
            const firstChanged = JSON.stringify(prevProofs[0]) !== JSON.stringify(newProofs[0]);
            const lastChanged = JSON.stringify(prevProofs[prevProofs.length - 1]) !== JSON.stringify(newProofs[newProofs.length - 1]);
            
            if (!firstChanged && !lastChanged) {
              console.log('[PROOFS] No changes detected - skipping update');
              return prevProofs;
            }
          }
          
          console.log('[PROOFS] State updated - proof count:', prevProofs.length, '->', newProofs.length);
          return newProofs;
        });
      }
    } catch (error) {
      console.error('Error fetching proofs:', error);
    }
  }, [sessionId, proofsApiUrl]);

  // Fetch proofs manually (for manual refresh button) - shows loading state
  const fetchProofs = useCallback(async () => {
    if (!sessionId) return;
    
    setProofLoading(true);
    try {
      const response = await axios.get(`${proofsApiUrl}/${sessionId}`);
      if (response.data.success) {
        setProofs(response.data.proofs);
      }
    } catch (error) {
      console.error('Error fetching proofs:', error);
    } finally {
      setProofLoading(false);
    }
  }, [sessionId, proofsApiUrl]);

  // Fetch full proof for modal display
  const fetchFullProof = useCallback(async (proofId: string) => {
    setProofModalLoading(true);
    try {
      const response = await axios.get(`${verifyProofUrl}/${proofId}`);
      if (response.data.success && response.data.proof) {
        setSelectedProof(response.data.proof);
        setProofModalOpen(true);
      }
    } catch (error) {
      console.error('Error fetching full proof:', error);
      alert('Failed to fetch proof details');
    } finally {
      setProofModalLoading(false);
    }
  }, [verifyProofUrl]);

  // Poll for proofs every 2 seconds when showing proofs (using automatic fetch, no loading state)
  React.useEffect(() => {
    if (!showProofs) return;

    const interval = setInterval(fetchProofsAutomatically, 5000);
    return () => clearInterval(interval);
  }, [showProofs, fetchProofsAutomatically]);

  const handleSendMessage = async () => {
    if (!input.trim()) return;

    try {
      setLoading(true);

      // Ensure WebSocket is connected
      if (!socket || socket.readyState !== WebSocket.OPEN) {
        console.log('[WEBSOCKET] Not connected, attempting to connect...');
        connectWebSocket();

        // Wait a bit for connection
        await new Promise((resolve) => setTimeout(resolve, 1000));

        if (!socket || socket.readyState !== WebSocket.OPEN) {
          throw new Error('WebSocket connection failed');
        }
      }

      // Add user message to chat
      const userMessage: ChatMessage = { role: 'user', content: input };
      setMessages((prev) => [...prev, userMessage]);

      // Send message through WebSocket
      const payload = {
        message: input,
        session_id: sessionId,
      };

      console.log('[WEBSOCKET] Sending:', payload);
      socket.send(JSON.stringify(payload));
      setInput('');

    } catch (error: any) {
      console.error('[WEBSOCKET] Send error:', error);
      const errorMessage: ChatMessage = {
        role: 'assistant',
        content: `Error: ${error.message}`,
      };
      setMessages((prev) => [...prev, errorMessage]);
      setLoading(false);
    }
  };

  const ProofBadge = React.memo(({ proof, index }: { proof: CryptographicProof; index: number }) => {
    const proofKey = proof.proof_id || `${proof.tool_name}-${index}`;
    const isExpanded = expandedProofIds.has(proofKey);
    
    const toggleExpanded = () => {
      const newSet = new Set(expandedProofIds);
      if (isExpanded) {
        newSet.delete(proofKey);
      } else {
        newSet.add(proofKey);
      }
      setExpandedProofIds(newSet);
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
            <div style={{ fontWeight: 'bold', color: proof.verified ? '#22543d' : '#742a2a', fontSize: '0.95rem', display: 'flex', alignItems: 'center', gap: '0.5rem' }}>
              <span style={{ fontSize: '1rem', transition: 'transform 0.2s ease', transform: isExpanded ? 'rotate(90deg)' : 'rotate(0deg)' }}>▶</span>
              {proof.sequence && <span style={{ marginRight: '0.5rem', background: stageBorder, color: '#fff', padding: '0.2rem 0.4rem', borderRadius: '0.25rem', fontSize: '0.75rem' }}>#{proof.sequence}</span>}
              🔐 {proof.verified ? '✓ Verified' : '✗ Unverified'} Proof
            </div>
            <div style={{ marginTop: '0.25rem', color: '#4a5568' }}>
              <strong>Tool:</strong> {proof.tool_name}
            </div>
            {proof.workflow_stage && (
              <div style={{ marginTop: '0.25rem', color: '#4a5568' }}>
                <strong>Stage:</strong> <span style={{ textTransform: 'uppercase', fontSize: '0.75rem', background: stageBorder, color: '#fff', padding: '0.1rem 0.3rem', borderRadius: '0.2rem' }}>{proof.workflow_stage}</span>
              </div>
            )}
            {!isExpanded && proof.proof_id && (
              <div 
                onClick={(e) => {
                  e.stopPropagation();
                  fetchFullProof(proof.proof_id!);
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
                <strong>ID:</strong> {proof.proof_id.substring(0, 32)}... <span style={{ fontSize: '0.65rem' }}>🔍 click to verify</span>
              </div>
            )}
            <div style={{ marginTop: '0.25rem', color: '#4a5568' }}>
              <strong>On-chain:</strong> {proof.onchain_compatible ? '✓ Yes' : '✗ No'}
            </div>
          </div>
        </div>
        
        {proof.related_proof_id && (
          <div style={{ marginTop: '0.25rem', padding: '0.25rem', background: 'rgba(0,0,0,0.02)', borderRadius: '0.2rem', fontSize: '0.75rem', color: '#4a5568' }}>
            ↳ Related to: {proof.related_proof_id.substring(0, 16)}...
          </div>
        )}

        {/* Expanded Details */}
        {isExpanded && (
          <div style={{ marginTop: '0.75rem', paddingTop: '0.75rem', borderTop: `1px solid ${stageBorder}`, animation: 'fadeIn 0.2s ease' }}>
            <div style={{ background: 'rgba(0,0,0,0.02)', padding: '0.75rem', borderRadius: '0.35rem', fontSize: '0.8rem', fontFamily: 'monospace', overflowX: 'auto' }}>
              <div style={{ marginBottom: '0.5rem' }}>
                <strong style={{ color: '#2d3748' }}>Proof ID:</strong>
                <div style={{ marginTop: '0.25rem', color: '#4a5568', wordBreak: 'break-all', background: '#fff', padding: '0.35rem', borderRadius: '0.25rem', display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                  <span>{proof.proof_id || 'Not available'}</span>
                  {proof.proof_id && (
                    <button
                      onClick={(e) => {
                        e.stopPropagation();
                        fetchFullProof(proof.proof_id!);
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
                <div style={{ marginTop: '0.25rem', color: '#4a5568', background: '#fff', padding: '0.35rem', borderRadius: '0.25rem' }}>
                  {new Date(proof.timestamp * 1000).toLocaleString()}
                </div>
              </div>

              <div style={{ marginBottom: '0.5rem' }}>
                <strong style={{ color: '#2d3748' }}>Verified:</strong>
                <div style={{ marginTop: '0.25rem', color: proof.verified ? '#22543d' : '#742a2a', background: '#fff', padding: '0.35rem', borderRadius: '0.25rem' }}>
                  {proof.verified ? '✓ Yes (Cryptographically signed by Reclaim)' : '✗ No (Proof validation pending)'}
                </div>
              </div>

              <div style={{ marginBottom: '0.5rem' }}>
                <strong style={{ color: '#2d3748' }}>On-Chain Compatible:</strong>
                <div style={{ marginTop: '0.25rem', color: '#4a5568', background: '#fff', padding: '0.35rem', borderRadius: '0.25rem' }}>
                  {proof.onchain_compatible ? '✓ Yes (Can be submitted to blockchain)' : '✗ No (Requires additional processing)'}
                </div>
              </div>

              {proof.workflow_stage && (
                <div style={{ marginBottom: '0.5rem' }}>
                  <strong style={{ color: '#2d3748' }}>Workflow Stage:</strong>
                  <div style={{ marginTop: '0.25rem', color: '#4a5568', background: '#fff', padding: '0.35rem', borderRadius: '0.25rem', textTransform: 'capitalize' }}>
                    {proof.workflow_stage}
                  </div>
                </div>
              )}

              {proof.sequence && (
                <div style={{ marginBottom: '0.5rem' }}>
                  <strong style={{ color: '#2d3748' }}>Sequence Number:</strong>
                  <div style={{ marginTop: '0.25rem', color: '#4a5568', background: '#fff', padding: '0.35rem', borderRadius: '0.25rem' }}>
                    {proof.sequence}
                  </div>
                </div>
              )}

              {proof.related_proof_id && (
                <div style={{ marginBottom: '0.5rem' }}>
                  <strong style={{ color: '#2d3748' }}>Related Proof ID:</strong>
                  <div style={{ marginTop: '0.25rem', color: '#4a5568', background: '#fff', padding: '0.35rem', borderRadius: '0.25rem', wordBreak: 'break-all' }}>
                    {proof.related_proof_id}
                  </div>
                </div>
              )}

              <div style={{ marginTop: '0.75rem', padding: '0.5rem', background: '#e6f0ff', borderRadius: '0.25rem', fontSize: '0.75rem', color: '#2c5282', lineHeight: '1.4' }}>
                <strong>What this proves:</strong>
                <div style={{ marginTop: '0.25rem' }}>
                  ✓ Agent-A made an authenticated HTTPS request to the {proof.tool_name} endpoint<br/>
                  ✓ The response data is genuine and cryptographically verified (Zero-Knowledge TLS)<br/>
                  ✓ No intermediary could have tampered with the data<br/>
                  {proof.onchain_compatible && '✓ This proof can be stored permanently on blockchain for audit trail'}
                </div>
              </div>
            </div>
          </div>
        )}
      </div>
    );
  }, (prevProps, nextProps) => {
    // Return true if props haven't changed (skip re-render)
    // Return false if props have changed (do re-render)
    return JSON.stringify(prevProps.proof) === JSON.stringify(nextProps.proof) && 
           prevProps.index === nextProps.index;
  });

  // Proof Modal Component - memoized to prevent re-renders
  const ProofModal = React.memo(() => {
    if (!proofModalOpen || !selectedProof) return null;

    return (
      <div
        style={{
          position: 'fixed',
          top: 0,
          left: 0,
          right: 0,
          bottom: 0,
          background: 'rgba(0, 0, 0, 0.5)',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          zIndex: 1000,
          animation: 'fadeIn 0.2s ease',
        }}
        onClick={() => setProofModalOpen(false)}
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
              onClick={() => setProofModalOpen(false)}
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
                  {selectedProof.onchain_compatible ? '✓ On-Chain Ready' : '✗ Not Ready'}
                </strong>
                <p style={{ margin: '0.25rem 0 0 0', fontSize: '0.85rem', color: selectedProof.onchain_compatible ? '#047857' : '#991b1b' }}>
                  {selectedProof.onchain_compatible ? 'Can submit to blockchain' : 'Requires additional processing'}
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
            <h3 style={{ color: '#2d3748', marginBottom: '0.75rem' }}>ZK-TLS Proof (Reclaim Protocol)</h3>
            <details style={{ cursor: 'pointer' }}>
              <summary style={{ padding: '0.5rem', background: '#f7fafc', borderRadius: '0.25rem', userSelect: 'none' }}>
                <strong>Click to expand proof data</strong> (for on-chain verification)
              </summary>
              <pre style={{ background: '#f7fafc', padding: '0.75rem', borderRadius: '0.25rem', fontSize: '0.75rem', overflow: 'auto', maxHeight: '300px', marginTop: '0.5rem' }}>
                {JSON.stringify(selectedProof.proof, null, 2)}
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
            onClick={() => setProofModalOpen(false)}
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

  return (
    <div className="app-container" style={{ height: '100vh', display: 'flex', flexDirection: 'column' }}>
      <header style={{ padding: '1rem', background: '#2d3748', color: '#fff', display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
        <div>
          <h1>AI Agent Chat Interface</h1>
          <p style={{ margin: '0.5rem 0 0 0', fontSize: '0.9rem', opacity: 0.8 }}>
            Ask about travel bookings, payments, and cryptographic proofs
          </p>
        </div>
        <div style={{ display: 'flex', gap: '0.5rem', alignItems: 'center' }}>
          <button
            onClick={() => setShowProofs(!showProofs)}
            style={{
              padding: '0.5rem 1rem',
              background: showProofs ? '#48bb78' : '#4299e1',
              color: '#fff',
              border: 'none',
              borderRadius: '0.25rem',
              cursor: 'pointer',
              fontSize: '0.9rem',
            }}
          >
            {showProofs ? '🔐 Hide' : '🔐 Show'} Proofs ({proofs.length})
          </button>
          {showProofs && (
            <button
              onClick={fetchProofs}
              disabled={proofLoading}
              style={{
                padding: '0.5rem 1rem',
                background: proofLoading ? '#cbd5e0' : '#667eea',
                color: '#fff',
                border: 'none',
                borderRadius: '0.25rem',
                cursor: proofLoading ? 'not-allowed' : 'pointer',
                fontSize: '0.9rem',
              }}
            >
              {proofLoading ? '⏳' : '🔄'} Refresh
            </button>
          )}
        </div>
      </header>

      <main style={{ flex: 1, overflowY: 'auto', padding: '1rem', background: '#f7fafc', display: 'flex', gap: '1rem' }}>
        <div style={{ flex: showProofs ? 2 : 1, minWidth: 0 }}>
          <div style={{ maxWidth: '900px', margin: '0 auto' }}>
            {messages.length === 0 && (
              <div
                style={{
                  textAlign: 'center',
                  padding: '2rem',
                  color: '#718096',
                }}
              >
                <h2>Welcome to AI Agent Chat</h2>
                <p>Start a conversation to get started</p>
                <p style={{ fontSize: '0.9rem', marginTop: '1rem' }}>
                  This interface collects cryptographic proofs of all Agent-B calls for on-chain verification
                </p>
              </div>
            )}

            {messages.map((msg, idx) => (
              <div
                key={idx}
                style={{
                  marginBottom: '1rem',
                  padding: '1rem',
                  background: msg.role === 'user' ? '#e2e8f0' : '#fff',
                  borderRadius: '0.5rem',
                  borderLeft: `4px solid ${msg.role === 'user' ? '#4299e1' : '#48bb78'}`,
                }}
              >
                <strong style={{ color: msg.role === 'user' ? '#2c5282' : '#22543d' }}>
                  {msg.role === 'user' ? 'You' : 'Agent'}
                </strong>
                <p style={{ margin: '0.5rem 0 0 0', whiteSpace: 'pre-wrap', wordWrap: 'break-word' }}>
                  {msg.content}
                </p>
              </div>
            ))}

            {loading && (
              <div style={{ padding: '1rem', textAlign: 'center', color: '#718096' }}>
                Agent is thinking...
              </div>
            )}
            <div ref={messagesEndRef} style={{ minHeight: '1px' }} />
          </div>
        </div>

        {showProofs && (
          <div
            style={{
              flex: 1,
              background: '#fff',
              borderRadius: '0.5rem',
              padding: '1rem',
              overflowY: 'auto',
              borderLeft: '2px solid #9f7aea',
            }}
          >
            <h3 style={{ marginTop: 0, color: '#5a67d8', display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
              <span>📊 Workflow Timeline</span>
              <span style={{ fontSize: '0.75rem', background: '#e6f0ff', color: '#2c5282', padding: '0.25rem 0.5rem', borderRadius: '0.25rem' }}>{proofs.length} proofs</span>
            </h3>
            
            {proofLoading && proofs.length === 0 && (
              <div style={{ color: '#718096', textAlign: 'center', padding: '1rem' }}>
                ⏳ Loading proofs...
              </div>
            )}
            {proofs.length === 0 && !proofLoading && (
              <div style={{ color: '#718096', fontSize: '0.9rem', textAlign: 'center', padding: '1.5rem 0.5rem' }}>
                <div style={{ marginBottom: '0.5rem' }}>No proofs yet</div>
                <div style={{ fontSize: '0.8rem', opacity: 0.7 }}>
                  Send a message to start collecting proofs from Agent-B
                </div>
              </div>
            )}

            {proofs.length > 0 && (
              <div style={{ position: 'relative', paddingLeft: '1.5rem' }}>
                {/* Timeline line */}
                <div style={{
                  position: 'absolute',
                  left: '0.25rem',
                  top: '0.5rem',
                  bottom: 0,
                  width: '2px',
                  background: 'linear-gradient(to bottom, #4299e1, #48bb78)',
                }} />
                
                {proofs.map((proof, idx) => (
                  <div key={proof.proof_id || `proof-${idx}`} style={{ position: 'relative', marginBottom: '1rem' }}>
                    {/* Timeline dot */}
                    <div style={{
                      position: 'absolute',
                      left: '-1.25rem',
                      top: '0.5rem',
                      width: '1.2rem',
                      height: '1.2rem',
                      borderRadius: '50%',
                      background: proof.verified ? '#48bb78' : '#f56565',
                      border: '3px solid #fff',
                      boxShadow: '0 0 0 2px ' + (proof.verified ? '#22543d' : '#742a2a'),
                      display: 'flex',
                      alignItems: 'center',
                      justifyContent: 'center',
                      fontSize: '0.6rem',
                      color: '#fff',
                      fontWeight: 'bold',
                    }}>
                      {proof.verified ? '✓' : '✗'}
                    </div>

                    {/* Proof content */}
                    <ProofBadge proof={proof} index={idx} />
                  </div>
                ))}
              </div>
            )}

            {proofs.length > 0 && (
              <div style={{ marginTop: '1rem', paddingTop: '1rem', borderTop: '1px solid #e2e8f0' }}>
                <div style={{ fontSize: '0.8rem', color: '#718096' }}>
                  <strong>Legend:</strong>
                  <div style={{ marginTop: '0.25rem' }}>✓ = Verified Proof | ✗ = Unverified Proof</div>
                </div>
              </div>
            )}
          </div>
        )}
      </main>

      <footer style={{ padding: '1rem', background: '#2d3748', borderTop: '1px solid #e2e8f0' }}>
        <div style={{ maxWidth: showProofs ? '1200px' : '900px', margin: '0 auto', display: 'flex', gap: '0.5rem' }}>
          <input
            type="text"
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyPress={(e) => e.key === 'Enter' && handleSendMessage()}
            placeholder="Type your message..."
            disabled={loading}
            style={{
              flex: 1,
              padding: '0.75rem',
              borderRadius: '0.25rem',
              border: '1px solid #cbd5e0',
              fontSize: '1rem',
            }}
          />
          <button
            onClick={handleSendMessage}
            disabled={loading || !input.trim()}
            style={{
              padding: '0.75rem 1.5rem',
              background: '#4299e1',
              color: '#fff',
              border: 'none',
              borderRadius: '0.25rem',
              cursor: loading ? 'not-allowed' : 'pointer',
              opacity: loading ? 0.6 : 1,
              fontSize: '1rem',
            }}
          >
            Send
          </button>
        </div>
      </footer>

      {/* Proof Modal */}
      {React.useMemo(() => <ProofModal />, [proofModalOpen, selectedProof])}
    </div>
  );
};

const App: React.FC = () => {
  return <ChatInterface />;
};

export default App;