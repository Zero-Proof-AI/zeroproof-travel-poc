import React, { createContext, useState, useEffect, useCallback } from 'react';

export interface CryptographicProof {
  tool_name: string;
  timestamp: number;
  proof_id?: string;
  verified: boolean;
  onchain_compatible: boolean;
  sequence?: number;
  related_proof_id?: string;
  workflow_stage?: string;
  submitted_by?: string;
  // Full proof data (when available)
  request?: any;
  response?: any;
  proof?: any;
  session_id?: string;
}

export interface ProofsContextType {
  proofs: CryptographicProof[];
  loading: boolean;
  error: string | null;
  fetchFullProof: (proofId: string) => Promise<CryptographicProof | null>;
}

export const ProofsContext = createContext<ProofsContextType | undefined>(undefined);

interface ProofsProviderProps {
  children: React.ReactNode;
  attestationServiceUrl: string;
  sessionId?: string;
}

export const ProofsProvider: React.FC<ProofsProviderProps> = ({
  children,
  attestationServiceUrl,
  sessionId,
}) => {
  const [proofs, setProofs] = useState<CryptographicProof[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [socket, setSocket] = useState<WebSocket | null>(null);

  // Connect to proofs WebSocket when sessionId is available
  useEffect(() => {
    if (!sessionId) {
      console.log('[PROOFS_CONTEXT] No sessionId yet, waiting...');
      return;
    }

    const wsUrl = attestationServiceUrl
      .replace('https://', 'wss://')
      .replace('http://', 'ws://');

    console.log('[PROOFS_CONTEXT] Connecting to', `${wsUrl}/ws/proofs?sessionId=${sessionId}`);

    const ws = new WebSocket(`${wsUrl}/ws/proofs?sessionId=${sessionId}`);

    ws.onopen = () => {
      console.log('[PROOFS_CONTEXT] ✅ Connected to proofs endpoint');
      setLoading(false);
      setError(null);
    };

    ws.onerror = (error) => {
      console.error('[PROOFS_CONTEXT] ❌ WebSocket error:', error);
      setError('Failed to connect to proofs service');
      setLoading(false);
    };

    ws.onclose = (event) => {
      console.warn('[PROOFS_CONTEXT] 🔌 WebSocket closed. Code:', event.code, 'Reason:', event.reason);
      setLoading(false);
    };

    ws.onmessage = (event) => {
      try {
        const data = JSON.parse(event.data);
        console.log('[PROOFS_CONTEXT] Received:', data);

        // Handle proof messages from attestation service
        if (data.proofs && Array.isArray(data.proofs)) {
          console.log('[PROOFS_CONTEXT] Proofs received:', data.proofs.length);
          // Append new proofs to existing ones
          setProofs((prevProofs) => {
            // Avoid duplicates by checking proof_id
            const proofIds = new Set(prevProofs.map((p) => p.proof_id));
            const newProofs = data.proofs.filter(
              (p: CryptographicProof) => !proofIds.has(p.proof_id)
            );
            return [...prevProofs, ...newProofs];
          });
        }

        // Handle single proof message
        if (data.proof && typeof data.proof === 'object') {
          console.log('[PROOFS_CONTEXT] Single proof received');
          setProofs((prevProofs) => {
            // Check if proof already exists (by proof_id)
            const exists = prevProofs.some((p) => p.proof_id === data.proof.proof_id);
            if (exists) {
              // Update existing proof
              return prevProofs.map((p) =>
                p.proof_id === data.proof.proof_id ? data.proof : p
              );
            }
            // Add new proof
            return [...prevProofs, data.proof];
          });
        }

        if (data.error) {
          console.error('[PROOFS_CONTEXT] Error from server:', data.error);
          setError(data.error);
        }
      } catch (err) {
        console.error('[PROOFS_CONTEXT] Failed to parse message:', err);
      }
    };

    ws.onerror = (err) => {
      console.error('[PROOFS_CONTEXT] WebSocket error:', err);
      setError('Failed to connect to proofs service');
    };

    ws.onclose = () => {
      console.log('[PROOFS_CONTEXT] Disconnected from proofs endpoint');
      setSocket(null);
    };

    setSocket(ws);

    // Cleanup on unmount or sessionId change
    return () => {
      if (ws.readyState === WebSocket.OPEN) {
        ws.close();
      }
    };
  }, [sessionId, attestationServiceUrl]);

  // Fetch full proof details (from local cache or API)
  const fetchFullProof = useCallback(
    async (proofId: string): Promise<CryptographicProof | null> => {
      console.log('[PROOFS_CONTEXT] Fetching full proof:', proofId);

      // First check local cache
      const localProof = proofs.find((p) => p.proof_id === proofId);
      if (
        localProof &&
        'request' in localProof &&
        'response' in localProof &&
        'proof' in localProof
      ) {
        console.log('[PROOFS_CONTEXT] Found complete proof in local cache');
        return localProof;
      }

      // Fallback to API call
      try {
        const verifyUrl = `${attestationServiceUrl}/proofs/verify/${proofId}`;
        const response = await fetch(verifyUrl);

        if (response.ok) {
          const data = await response.json();
          if (data.proof) {
            console.log('[PROOFS_CONTEXT] Fetched full proof from API');
            // Update local proofs with full data
            setProofs((prevProofs) =>
              prevProofs.map((p) => (p.proof_id === proofId ? data.proof : p))
            );
            return data.proof;
          }
        }
      } catch (err) {
        console.error('[PROOFS_CONTEXT] Error fetching full proof:', err);
      }

      return null;
    },
    [proofs, attestationServiceUrl]
  );

  const value: ProofsContextType = {
    proofs,
    loading,
    error,
    fetchFullProof,
  };

  return <ProofsContext.Provider value={value}>{children}</ProofsContext.Provider>;
};
