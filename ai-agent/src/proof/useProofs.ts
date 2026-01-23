import { useContext } from 'react';
import { ProofsContext, ProofsContextType } from './ProofsContext';

export const useProofs = (): ProofsContextType => {
  const context = useContext(ProofsContext);
  if (!context) {
    throw new Error('useProofs must be used within a ProofsProvider');
  }
  return context;
};
