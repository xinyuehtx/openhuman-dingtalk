import { Navigate } from 'react-router-dom';

import { useCoreState } from '../providers/CoreStateProvider';
import { hasStoredLlmSettings } from '../utils/configPersistence';
import RouteLoadingScreen from './RouteLoadingScreen';

interface ProtectedRouteProps {
  children: React.ReactNode;
  requireAuth?: boolean;
  redirectTo?: string;
}

/**
 * Protected route component that handles authentication checks.
 *
 * Access is granted when the user has either:
 *  - A valid session token (original OAuth flow), OR
 *  - Configured LLM settings (custom LLM flow for DingTalk fork).
 *
 * Onboarding gating is handled by the AppShell effect (see App.tsx)
 * which redirects between `/onboarding` and the rest of the app based
 * on `onboarding_completed`.
 */
const ProtectedRoute = ({ children, requireAuth = true, redirectTo }: ProtectedRouteProps) => {
  const { isBootstrapping, snapshot } = useCoreState();

  if (isBootstrapping) {
    return <RouteLoadingScreen />;
  }

  const isAuthenticated = !!snapshot.sessionToken || hasStoredLlmSettings();

  if (requireAuth && !isAuthenticated) {
    return <Navigate to={redirectTo || '/'} replace />;
  }

  return <>{children}</>;
};

export default ProtectedRoute;
