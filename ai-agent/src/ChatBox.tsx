import React from 'react';

interface ChatMessage {
  role: string;
  content: string;
}

interface ChatBoxProps {
  messages: ChatMessage[];
  loading: boolean;
  messagesEndRef: React.RefObject<HTMLDivElement | null>;
}

const ChatBox: React.FC<ChatBoxProps> = ({ messages, loading, messagesEndRef }) => {
  // Auto-scroll to bottom when messages change
  const scrollToBottom = () => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  };

  React.useEffect(() => {
    scrollToBottom();
  }, [messages, loading]);
  return (
    <div style={{ maxWidth: '900px', margin: '0 auto' }}>
      {messages.length === 0 && (
        <div
          style={{
            textAlign: 'center',
            padding: '2rem',
            color: '#718096',
          }}
        >
          <h2>Looking for your next adventure? Go no further!</h2>
          <p>Start a conversation to get started</p>
          <p style={{ fontSize: '0.9rem', color: '#0dc39f', marginTop: '1rem' }}>
            <strong >
              Never lose money from an agentic transaction.
            </strong>
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
  );
};

export default ChatBox;
