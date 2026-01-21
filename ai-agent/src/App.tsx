import React, { useState, useCallback } from 'react';
import axios from 'axios';
import './App.css';
import ProgressPanel from './ProgressPanel';
import ProofBadge from './ProofBadge';
import ProofModal from './ProofModal';
import ChatBox from './ChatBox';

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
  const [progressUpdateTrigger, setProgressUpdateTrigger] = useState(0); // Trigger re-renders when progress updates
  const progressMessagesRef = React.useRef<string[]>([]);
  const messagesEndRef = React.useRef<HTMLDivElement>(null);
  const progressPanelRef = React.useRef<HTMLDivElement>(null);
  const inputRef = React.useRef<HTMLInputElement>(null);

  // Add animation styles
  React.useEffect(() => {
    const style = document.createElement('style');
    style.textContent = `
      @keyframes slideIn {
        from {
          opacity: 0;
          transform: translateX(-10px);
        }
        to {
          opacity: 1;
          transform: translateX(0);
        }
      }
      @keyframes fadeIn {
        from { opacity: 0; }
        to { opacity: 1; }
      }
    `;
    document.head.appendChild(style);
    return () => {
      document.head.removeChild(style);
    };
  }, []);

  // Backend URLs - point to Agent A HTTP/WebSocket Server
  // For local: http://localhost:3001, ws://localhost:3001
  // For production: https://dev.agenta.zeroproofai.com, wss://dev.agenta.zeroproofai.com
  
  
  // Local environment
  // const baseUrl = 'http://localhost:3001';
  // Test Environment
  const baseUrl = 'https://dev.agenta.zeroproofai.com';
 
  const wsUrl = baseUrl.replace('https://', 'wss://').replace('http://', 'ws://');
  const proofsApiUrl = `${baseUrl}/proofs`;
  const verifyProofUrl = `${baseUrl}/proofs/verify`;

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

        // Handle proof messages injected from backend
        if (data.proofs && Array.isArray(data.proofs)) {
          console.log('[WEBSOCKET] Proofs received:', data.proofs.length, 'proofs');
          // Append new proofs to existing ones instead of replacing
          setProofs(prevProofs => [...prevProofs, ...data.proofs]);
        }

        // Handle progress messages (real-time updates) - display them immediately
        if (data.progress && !data.response) {
          console.log('[WEBSOCKET] Progress update:', data.progress);
          progressMessagesRef.current.push(data.progress);
          
          // Trigger re-render of progress panel
          setProgressUpdateTrigger(prev => prev + 1);
          
          // Auto-scroll to show latest progress
          if (progressPanelRef.current) {
            setTimeout(() => {
              if (progressPanelRef.current) {
                progressPanelRef.current.scrollTop = progressPanelRef.current.scrollHeight;
              }
            }, 50);
          }
        }
        // Handle final response messages
        if (data.success && data.response) {
          console.log('[WEBSOCKET] Final response:', data.response);
          
          // Add the final response as a NEW separate message (don't mix with progress)
          setMessages((prev) => {
            const updated = [...prev];
            updated.push({
              role: 'assistant',
              content: data.response,
            });
            return updated;
          });
          
          // Keep input disabled for a moment to prevent too-fast responses
          setTimeout(() => {
            setLoading(false);
            // Auto-focus input field for convenience
            setTimeout(() => {
              inputRef.current?.focus();
            }, 100);
          }, 800);
          
          // Fetch proofs after agent responds - reserved for later, need to fetch proofs from attester
          // console.log('[WEBSOCKET] Agent responded, triggering proof fetch for session:', data.session_id);
          // setTimeout(() => {
          //   if (data.session_id) {
          //     fetchProofsWithSession(data.session_id);
          //   }
          // }, 500);
        } else if (data.error) {
          console.error('[WEBSOCKET] Error from server:', data.error);
          
          // Replace the last message with error
          setMessages((prev) => {
            const updated = [...prev];
            const errorContent = `Error: ${data.error || 'Unknown error'}`;
            
            if (updated.length > 0 && updated[updated.length - 1].role === 'assistant') {
              // Replace the last message with error
              updated[updated.length - 1] = {
                role: 'assistant',
                content: errorContent,
              };
            } else {
              // Add error as new message
              updated.push({
                role: 'assistant',
                content: errorContent,
              });
            }
            return updated;
          });
          
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
  // const fetchProofsAutomatically = useCallback(async () => {
  //   if (!sessionId) {
  //     console.log('[PROOFS] Skipping fetch - no sessionId');
  //     return;
  //   }
    
  //   console.log('[PROOFS] Fetching proofs for sessionId:', sessionId);
  //   try {
  //     const response = await axios.get(`${proofsApiUrl}/${sessionId}`);
  //     if (response.data.success) {
  //       // Only update if proofs actually changed
  //       setProofs((prevProofs) => {
  //         const newProofs = response.data.proofs;
          
  //         // Quick check: if count is the same, likely no changes
  //         if (prevProofs.length === newProofs.length && prevProofs.length > 0) {
  //           // For same-length arrays, compare only the first and last items to detect changes
  //           const firstChanged = JSON.stringify(prevProofs[0]) !== JSON.stringify(newProofs[0]);
  //           const lastChanged = JSON.stringify(prevProofs[prevProofs.length - 1]) !== JSON.stringify(newProofs[newProofs.length - 1]);
            
  //           if (!firstChanged && !lastChanged) {
  //             console.log('[PROOFS] No changes detected - skipping update');
  //             return prevProofs;
  //           }
  //         }
          
  //         console.log('[PROOFS] State updated - proof count:', prevProofs.length, '->', newProofs.length);
  //         return newProofs;
  //       });
  //     }
  //   } catch (error) {
  //     console.error('Error fetching proofs:', error);
  //   }
  // }, [sessionId, proofsApiUrl]);

  // // Fetch proofs with specific session ID (for immediate use after WebSocket message)
  // const fetchProofsWithSession = useCallback(async (targetSessionId: string) => {
  //   console.log('[PROOFS] Fetching proofs for specific sessionId:', targetSessionId);
  //   try {
  //     const response = await axios.get(`${proofsApiUrl}/${targetSessionId}`);
  //     if (response.data.success) {
  //       setProofs(response.data.proofs);
  //       console.log('[PROOFS] Fetched and set proofs:', response.data.proofs.length);
  //     }
  //   } catch (error) {
  //     console.error('Error fetching proofs:', error);
  //   }
  // }, [proofsApiUrl]);

  // // Fetch proofs manually (for manual refresh button) - shows loading state
  // const fetchProofs = useCallback(async () => {
  //   if (!sessionId) return;
    
  //   setProofLoading(true);
  //   try {
  //     const response = await axios.get(`${proofsApiUrl}/${sessionId}`);
  //     if (response.data.success) {
  //       setProofs(response.data.proofs);
  //     }
  //   } catch (error) {
  //     console.error('Error fetching proofs:', error);
  //   } finally {
  //     setProofLoading(false);
  //   }
  // }, [sessionId, proofsApiUrl]);

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
  // React.useEffect(() => {
  //   if (!showProofs) return;

  //   const interval = setInterval(fetchProofsAutomatically, 5000);
  //   return () => clearInterval(interval);
  // }, [showProofs, fetchProofsAutomatically]);

  const handleSendMessage = async () => {
    if (!input.trim()) return;

    try {
      setLoading(true);
      // Add separator between requests instead of clearing - keeps full history
      if (progressMessagesRef.current.length > 0) {
        progressMessagesRef.current.push('---');
      }
      setProgressUpdateTrigger(prev => prev + 1); // Trigger re-render

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
            <span
              style={{
                fontSize: '0.9rem',
                color: '#666',
                fontStyle: 'italic',
              }}
            >
              (Live via WebSocket)
            </span>
          )}
        </div>
      </header>

      <main style={{ flex: 1, overflow: 'hidden', padding: '1rem', background: '#f7fafc', display: 'flex', gap: '1rem' }}>
        <div style={{ flex: showProofs ? 2 : 1, minWidth: 0, overflowY: 'auto' }}>
          <ChatBox
            messages={messages}
            loading={loading}
            messagesEndRef={messagesEndRef}
          />
        </div>

        <div style={{ flex: showProofs ? 1 : 0.8, minWidth: 0, maxWidth: '300px', display: 'flex', flexDirection: 'column' }}>
          <ProgressPanel
            ref={progressPanelRef}
            progressMessages={progressMessagesRef.current}
            updateTrigger={progressUpdateTrigger}
          />
        </div>

        {showProofs && (
          <div
            style={{
              flex: 1,
              background: '#fff',
              borderRadius: '0.5rem',
              padding: '1rem',
              overflowY: 'auto',
              borderLeft: '1px solid #cbd5e0',
              boxShadow: '0 1px 3px rgba(0,0,0,0.1)',
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
                    <ProofBadge
                      proof={proof}
                      index={idx}
                      expandedProofIds={expandedProofIds}
                      onToggleExpanded={(proofKey) => {
                        const newSet = new Set(expandedProofIds);
                        if (newSet.has(proofKey)) {
                          newSet.delete(proofKey);
                        } else {
                          newSet.add(proofKey);
                        }
                        setExpandedProofIds(newSet);
                      }}
                      onFetchFullProof={fetchFullProof}
                      proofModalLoading={proofModalLoading}
                    />
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
            ref={inputRef}
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
      <ProofModal
        open={proofModalOpen}
        selectedProof={selectedProof}
        onClose={() => setProofModalOpen(false)}
      />
    </div>
  );
};

const App: React.FC = () => {
  return <ChatInterface />;
};

export default App;