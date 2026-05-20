import createDebug from 'debug';
import { useCallback } from 'react';
import { useNavigate } from 'react-router-dom';

import type { LlmSettings } from '../utils/configPersistence';
import LlmSetup from './LlmSetup';

const log = createDebug('app:welcome');

/**
 * Welcome / login page — presents the LLM configuration form instead of
 * the original OAuth flow. Once the user saves valid LLM settings the app
 * navigates to `/home`.
 */
const Welcome = () => {
  const navigate = useNavigate();

  const handleLlmSetupComplete = useCallback(
    (settings: LlmSettings) => {
      log('LLM setup complete — navigating to /home', {
        inferenceUrl: settings.inferenceUrl,
        model: settings.model,
      });
      navigate('/home', { replace: true });
    },
    [navigate]
  );

  return <LlmSetup onComplete={handleLlmSetupComplete} />;
};

export default Welcome;
