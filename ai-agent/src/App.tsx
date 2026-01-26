import React, { useState } from 'react';
import './App.css';
import ProgressPanel from './ProgressPanel';
import { ProofBadge, ProofModal, ProofsProvider, useProofs, type FullProofData } from './proof';
import ChatBox from './ChatBox';

// Backend URLs
// const AI_AGENT_BASE_URL = 'http://localhost:3001';
// Test Environment
const AI_AGENT_BASE_URL = 'https://dev.agenta.zeroproofai.com';

interface ChatMessage {
  role: string;
  content: string;
}

// Toggle button for proofs visibility
const ProofsToggleButton: React.FC<{
  showProofs: boolean;
  setShowProofs: (show: boolean) => void;
}> = ({ showProofs, setShowProofs }) => {
  const { proofs } = useProofs();

  return (
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
          fontWeight: 500,
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
  );
};

// Proofs panel component
const ProofsPanel: React.FC = () => {
  const { proofs, loading, fetchFullProof } = useProofs();
  const [selectedProof, setSelectedProof] = useState<FullProofData | null>(null);
  const [proofModalOpen, setProofModalOpen] = useState(false);

  const handleFetchFullProof = async (proofId: string) => {
    console.log('[PROOFS_PANEL] User clicked to view full proof:', proofId);
    
    try {
      const fullProof = await fetchFullProof(proofId);
      if (fullProof) {
        console.log('[PROOFS_PANEL] Successfully fetched full proof data');
        setSelectedProof(fullProof as FullProofData);
        setProofModalOpen(true);
      } else {
        console.warn('[PROOFS_PANEL] Failed to fetch full proof data');
      }
    } catch (error) {
      console.error('[PROOFS_PANEL] Error fetching full proof:', error);
    }
  };

  return (
    <>
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
        <h4 style={{ marginTop: 0, color: '#5a67d8', display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
          <span>📊 Workflow Timeline</span>
          <span style={{ fontSize: '0.75rem', background: '#e6f0ff', color: '#2c5282', padding: '0.25rem 0.5rem', borderRadius: '0.25rem' }}>
            {proofs.length} proofs
          </span>
        </h4>

        {loading && proofs.length === 0 && (
          <div style={{ color: '#718096', textAlign: 'center', padding: '1rem' }}>
            ⏳ Loading proofs...
          </div>
        )}
        {proofs.length === 0 && !loading && (
          <div style={{ color: '#718096', fontSize: '0.9rem', textAlign: 'center', padding: '1.5rem 0.5rem' }}>
            <div style={{ marginBottom: '0.5rem' }}>No proofs yet</div>
            <div style={{ fontSize: '0.8rem', opacity: 0.7 }}>
              Send a message to start collecting proofs from all agents
            </div>
          </div>
        )}

        {proofs.length > 0 && (
          <div style={{ position: 'relative', paddingLeft: '1.5rem' }}>
            {/* Timeline line */}
            <div
              style={{
                position: 'absolute',
                left: '0.25rem',
                top: '0.5rem',
                bottom: 0,
                width: '2px',
                background: 'linear-gradient(to bottom, #4299e1, #48bb78)',
              }}
            />

            {proofs.map((proof, idx) => (
              <div key={proof.proof_id || `proof-${idx}`} style={{ position: 'relative', marginBottom: '1rem' }}>
                {/* Timeline dot */}
                <div
                  style={{
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
                  }}
                >
                  {proof.verified ? '✓' : '✗'}
                </div>

                {/* Proof content */}
                <ProofBadge proof={proof} index={idx} onFetchFullProof={handleFetchFullProof} />
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

      {/* Proof Modal */}
      <ProofModal open={proofModalOpen} selectedProof={selectedProof} onClose={() => setProofModalOpen(false)} />
    </>
  );
};

const ChatInterface: React.FC = () => {
  // Log immediately on component render
  console.log('[APP] ChatInterface component rendering');

  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [input, setInput] = useState('');
  const [loading, setLoading] = useState(false);
  const [sessionId, setSessionId] = useState<string>('');
  const [showProofs, setShowProofs] = useState(false);
  const [socket, setSocket] = useState<WebSocket | null>(null);
  const [wsConnected, setWsConnected] = useState(false);
  const [progressUpdateTrigger, setProgressUpdateTrigger] = useState(0);
  const progressMessagesRef = React.useRef<string[]>([]);
  const messagesEndRef = React.useRef<HTMLDivElement>(null);
  const progressPanelRef = React.useRef<HTMLDivElement>(null);
  const inputRef = React.useRef<HTMLInputElement>(null);
  const heartbeatIntervalRef = React.useRef<NodeJS.Timeout | null>(null);

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

  const wsUrl = AI_AGENT_BASE_URL.replace('https://', 'wss://').replace('http://', 'ws://');

  console.log('[APP] UI Agent Configuration:');
  console.log('[APP]   Base URL (HTTP):', AI_AGENT_BASE_URL);

  // WebSocket connection management for chat
  const connectWebSocket = () => {
    if (socket && socket.readyState === WebSocket.OPEN) {
      return;
    }

    console.log('[WEBSOCKET] Connecting to', wsUrl + '/ws/chat');
    const ws = new WebSocket(`${wsUrl}/ws/chat`);

    ws.onopen = () => {
      console.log('[WEBSOCKET] Connected');
      setWsConnected(true);
      setSocket(ws);

      // Start heartbeat to keep connection alive
      if (heartbeatIntervalRef.current) {
        clearInterval(heartbeatIntervalRef.current);
      }
      heartbeatIntervalRef.current = setInterval(() => {
        if (ws.readyState === WebSocket.OPEN) {
          console.log('[WEBSOCKET] Sending heartbeat');
          ws.send(JSON.stringify({ type: 'ping' }));
        }
      }, 30000); // Send heartbeat every 30 seconds
    };

    ws.onmessage = (event) => {
      try {
        const data = JSON.parse(event.data);
        console.log('[WEBSOCKET] Received:', data);

        // Store server-returned session ID
        if (data.session_id && data.session_id !== sessionId) {
          console.log('[WEBSOCKET] Updating session ID from server:', data.session_id);
          setSessionId(data.session_id);
        }

        // Handle progress messages (real-time updates)
        if (data.progress && !data.response) {
          console.log('[WEBSOCKET] Progress update:', data.progress);
          progressMessagesRef.current.push(data.progress);
          setProgressUpdateTrigger((prev) => prev + 1);

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

          setMessages((prev) => {
            const updated = [...prev];
            updated.push({
              role: 'assistant',
              content: data.response,
            });
            return updated;
          });

          setTimeout(() => {
            setLoading(false);
            setTimeout(() => {
              inputRef.current?.focus();
            }, 100);
          }, 800);
        } else if (data.error) {
          console.error('[WEBSOCKET] Error from server:', data.error);

          setMessages((prev) => {
            const updated = [...prev];
            const errorContent = `Error: ${data.error || 'Unknown error'}`;

            if (updated.length > 0 && updated[updated.length - 1].role === 'assistant') {
              updated[updated.length - 1] = {
                role: 'assistant',
                content: errorContent,
              };
            } else {
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
      if (heartbeatIntervalRef.current) {
        clearInterval(heartbeatIntervalRef.current);
        heartbeatIntervalRef.current = null;
      }
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
    if (heartbeatIntervalRef.current) {
      clearInterval(heartbeatIntervalRef.current);
      heartbeatIntervalRef.current = null;
    }
  };

  // Initialize WebSocket connection on mount
  React.useEffect(() => {
    connectWebSocket();
    return () => {
      disconnectWebSocket();
    };
  }, []);

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
    <ProofsProvider sessionId={sessionId}>
      <div className="app-container" style={{ height: '100vh', display: 'flex', flexDirection: 'column' }}>
        <header style={{ padding: '1rem', background: '#2d3748', color: '#fff', display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
          <div>
            <h1>AI Travel Agent</h1>
            <p style={{ margin: '0.5rem 0 0 0', fontSize: '0.9rem', opacity: 0.8 }}>
              Ask about travel bookings, payments, and cryptographic proofs
            </p>
          </div>
          <ProofsToggleButton showProofs={showProofs} setShowProofs={setShowProofs} />
        </header>

        <main style={{ flex: 1, overflow: 'hidden', padding: '1rem', background: '#f7fafc', display: 'flex', gap: '1rem' }}>
          <div style={{ flex: showProofs ? 2 : 1, minWidth: 0, overflow: 'hidden', display: 'flex', flexDirection: 'column' }}>
            <div style={{ flex: 1, overflowY: 'auto', marginBottom: '1rem' }}>
              <ChatBox messages={messages} loading={loading} messagesEndRef={messagesEndRef} />
            </div>
            
            <footer style={{ padding: '0.5rem', background: '#2d3748', borderTop: '1px solid #e2e8f0', borderRadius: '0.25rem', flexShrink: 0 }}>
              <div style={{ display: 'flex', gap: '0.5rem' }}>
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
                    padding: '0.5rem',
                    borderRadius: '0.25rem',
                    border: '1px solid #cbd5e0',
                    fontSize: '1rem',
                  }}
                />
                <button
                  onClick={handleSendMessage}
                  disabled={loading || !input.trim()}
                  style={{
                    padding: '0.5rem 1rem',
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
          </div>

          <div style={{ flex: showProofs ? 1 : 0.8, minWidth: 0, maxWidth: '300px', display: 'flex', flexDirection: 'column' }}>
            <ProgressPanel ref={progressPanelRef} progressMessages={progressMessagesRef.current} updateTrigger={progressUpdateTrigger} />
          </div>

          {showProofs && <ProofsPanel />}
        </main>
      </div>
    </ProofsProvider>
  );
};

const App: React.FC = () => {
  return <ChatInterface />;
};

export default App;