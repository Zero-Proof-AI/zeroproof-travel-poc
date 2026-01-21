import React from 'react';

interface ProgressPanelProps {
  progressMessages: string[];
  updateTrigger: number;
}

const ProgressPanel = React.forwardRef<HTMLDivElement, ProgressPanelProps>(
  ({ progressMessages, updateTrigger }, ref) => {
    // Auto-scroll to bottom when messages update
    React.useEffect(() => {
      if (ref && 'current' in ref && ref.current) {
        setTimeout(() => {
          if (ref.current) {
            ref.current.scrollTop = ref.current.scrollHeight;
          }
        }, 50);
      }
    }, [updateTrigger, ref]);

    return (
      <div
        ref={ref}
        style={{
          background: '#fff',
          borderRadius: '0.5rem',
          padding: '1rem',
          overflowY: 'auto',
          borderLeft: '1px solid #cbd5e0',
          flex: 1,
          boxShadow: '0 1px 3px rgba(0,0,0,0.1)',
        }}
      >
        <h3 style={{ marginTop: 0, marginBottom: '1rem', color: '#2d3748', fontSize: '0.95rem', fontWeight: '600' }}>
          📊 Progress ({progressMessages.length})
        </h3>

        {progressMessages.length === 0 ? (
          <div style={{ color: '#a0aec0', fontSize: '0.85rem', textAlign: 'center', padding: '1rem 0' }}>
            <div style={{ marginBottom: '0.5rem' }}>No progress yet</div>
            <div style={{ fontSize: '0.75rem', opacity: 0.7 }}>
              Send a message to start
            </div>
          </div>
        ) : (
          <div style={{ display: 'flex', flexDirection: 'column', gap: '0.75rem' }}>
            {progressMessages.map((progress, idx) => {
              // Handle separators between requests
              if (progress === '---') {
                return (
                  <div
                    key={idx}
                    style={{
                      margin: '0.5rem 0',
                      borderTop: '2px dashed #cbd5e0',
                      padding: '0.5rem 0',
                    }}
                  />
                );
              }

              return (
                <div
                  key={idx}
                  style={{
                    fontSize: '0.8rem',
                    whiteSpace: 'pre-wrap',
                    wordWrap: 'break-word',
                    lineHeight: '1.4',
                    color: '#4a5568',
                    animation: 'slideIn 0.3s ease',
                  }}
                >
                  {progress}
                </div>
              );
            })}
          </div>
        )}
      </div>
    );
  }
);

ProgressPanel.displayName = 'ProgressPanel';

export default ProgressPanel;
