import React, { useState } from 'react';
import axios from 'axios';
import './App.css';

const ChatInterface: React.FC = () => {
  const [messages, setMessages] = useState<{ role: string; content: string }[]>([]);
  const [input, setInput] = useState('');
  const [loading, setLoading] = useState(false);
  const [sessionId, setSessionId] = useState<string>('');

  // Point to Agent A HTTP Server (mcp-client-http)
  // Uses environment variable from ConfigMap (mounted at /config/env.js), falls back to build-time env or localhost
  const getBackendUrl = () => {
    // @ts-ignore - window.env is set by ConfigMap at runtime
    if (window.env?.REACT_APP_BACKEND_API) {
      // @ts-ignore
      return window.env.REACT_APP_BACKEND_API;
    }
    return process.env.REACT_APP_BACKEND_API || 'http://localhost:3001/chat';
  };
  
  const [backendApiUrl, setBackendApiUrl] = React.useState('http://localhost:3001/chat');

  // Load backend URL from ConfigMap config file or fallback to environment variable
  React.useEffect(() => {
    const loadConfig = async () => {
      try {
        // Try to load config from ConfigMap-mounted file
        const response = await fetch('/config/env.js');
        if (response.ok) {
          // Config file loaded successfully, window.env should be set
          setBackendApiUrl(getBackendUrl());
        } else {
          // Fall back to environment variable or localhost
          setBackendApiUrl(getBackendUrl());
        }
      } catch (error) {
        // Fall back to environment variable or localhost
        setBackendApiUrl(getBackendUrl());
      }
    };

    loadConfig();
  }, []);

  // Generate session ID on component mount
  React.useEffect(() => {
    const newSessionId = `sess_${Math.random().toString(36).substring(2, 15)}`;
    setSessionId(newSessionId);
  }, []);

  const handleSendMessage = async () => {
    if (!input.trim()) return;

    const userMessage = { role: 'user', content: input };
    setMessages((prev) => [...prev, userMessage]);
    setInput('');
    setLoading(true);

    try {
      const response = await axios.post(backendApiUrl, {
        message: input,
        session_id: sessionId,
      });

      const assistantMessage = {
        role: 'assistant',
        content: response.data.response || 'No response',
      };
      setMessages((prev) => [...prev, assistantMessage]);
    } catch (error) {
      console.error('Error sending message:', error);
      const errorMessage = {
        role: 'assistant',
        content: 'Error: Could not reach the backend service. Make sure mcp-client-http is running on port 3001.',
      };
      setMessages((prev) => [...prev, errorMessage]);
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="app-container" style={{ height: '100vh', display: 'flex', flexDirection: 'column' }}>
      <header style={{ padding: '1rem', background: '#2d3748', color: '#fff' }}>
        <h1>AI Agent Chat Interface</h1>
        <p style={{ margin: '0.5rem 0 0 0', fontSize: '0.9rem', opacity: 0.8 }}>
          Ask about travel bookings, payments, and more
        </p>
      </header>

      <main style={{ flex: 1, overflowY: 'auto', padding: '1rem', background: '#f7fafc' }}>
        <div style={{ maxWidth: '800px', margin: '0 auto' }}>
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
        </div>
      </main>

      <footer style={{ padding: '1rem', background: '#2d3748', borderTop: '1px solid #e2e8f0' }}>
        <div style={{ maxWidth: '800px', margin: '0 auto', display: 'flex', gap: '0.5rem' }}>
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
    </div>
  );
};

const App: React.FC = () => {
  return <ChatInterface />;
};

export default App;