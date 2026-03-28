import { Component, ErrorInfo, ReactNode } from 'react';

interface Props {
  children: ReactNode;
  fallback?: ReactNode;
}

interface State {
  hasError: boolean;
  error: Error | null;
  retryCount: number;
}

/**
 * Global React error boundary.
 * Catches render/lifecycle errors, logs them, and shows a recovery UI.
 */
export class ErrorBoundary extends Component<Props, State> {
  state: State = { hasError: false, error: null, retryCount: 0 };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { hasError: true, error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error('[ErrorBoundary]', error, info.componentStack);
    // Best-effort remote report
    try {
      navigator.sendBeacon?.(
        '/api/errors',
        JSON.stringify({
          message: error.message,
          stack: error.stack,
          componentStack: info.componentStack,
          ts: new Date().toISOString(),
        })
      );
    } catch { /* ignore */ }
  }

  private retry = () => {
    this.setState((s) => ({ hasError: false, error: null, retryCount: s.retryCount + 1 }));
  };

  render() {
    if (!this.state.hasError) return this.props.children;

    if (this.props.fallback) return this.props.fallback;

    return (
      <div role="alert" style={{ padding: '2rem', textAlign: 'center' }}>
        <h2>Something went wrong</h2>
        <p style={{ color: '#666', marginBottom: '1rem' }}>
          {friendlyMessage(this.state.error)}
        </p>
        {this.state.retryCount < 3 && (
          <button onClick={this.retry} style={{ marginRight: '0.5rem' }}>
            Try again
          </button>
        )}
        <button onClick={() => window.location.reload()}>Reload page</button>
      </div>
    );
  }
}

// ── Error message mapping ─────────────────────────────────────────────────────

const ERROR_MESSAGES: Record<string, string> = {
  // Contract error codes
  '3':  'Amount must be greater than zero.',
  '5':  'This arbitrator is not registered.',
  '6':  'Trade not found. Check the trade ID.',
  '7':  'This action is not allowed in the current trade state.',
  '10': 'You are not authorized to perform this action.',
  '11': 'The contract is currently paused.',
  // HTTP
  '400': 'Invalid request. Please check your input.',
  '401': 'Session expired. Please reconnect your wallet.',
  '403': 'You do not have permission to do that.',
  '404': 'The requested resource was not found.',
  '429': 'Too many requests. Please wait a moment.',
  '500': 'Server error. Our team has been notified.',
  '503': 'Service unavailable. You may be offline.',
};

export function friendlyMessage(error: Error | null): string {
  if (!error) return 'An unexpected error occurred. Please try again.';
  const code = (error as any).code;
  if (code !== undefined) return ERROR_MESSAGES[String(code)] ?? error.message;
  return error.message || 'An unexpected error occurred. Please try again.';
}
