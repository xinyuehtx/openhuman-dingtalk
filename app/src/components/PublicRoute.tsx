import { Navigate } from 'react-router-dom';

import { useCoreState } from '../providers/CoreStateProvider';
import { hasStoredLlmSettings } from '../utils/configPersistence';
import RouteLoadingScreen from './RouteLoadingScreen';

interface PublicRouteProps {
  children: React.ReactNode;
  redirectTo?: string;
}

/**
 * Public route component that redirects authenticated users to /home.
 *
 * "Authenticated" means the user has either a valid session token (original
 * OAuth flow) or has configured LLM settings (custom LLM flow for DingTalk
 * fork). Home handles the onboarding redirect once the user profile is loaded.
 */
const PublicRoute = ({ children, redirectTo }: PublicRouteProps) => {
  const { isBootstrapping, snapshot } = useCoreState();

  if (isBootstrapping) {
    return <RouteLoadingScreen />;
  }

  // If user is logged in or has configured LLM, go to home.
  if (snapshot.sessionToken || hasStoredLlmSettings()) {
    return <Navigate to={redirectTo || '/home'} replace />;
  }

  // User is not logged in and has no LLM config, show public route
  return <>{children}</>;
};

export default PublicRoute;
