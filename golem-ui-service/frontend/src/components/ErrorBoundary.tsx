import { AlertCircle, Home, RefreshCw } from 'lucide-react';
import { Component, ErrorInfo, ReactNode } from 'react';

interface Props {
    children: ReactNode;
    fallback?: ReactNode;
}

interface State {
    hasError: boolean;
    error: Error | null;
    errorInfo: ErrorInfo | null;
}

export class ErrorBoundary extends Component<Props, State> {
    public state: State = {
        hasError: false,
        error: null,
        errorInfo: null
    };

    public static getDerivedStateFromError(error: Error): State {
        return { hasError: true, error, errorInfo: null };
    }

    public componentDidCatch(error: Error, errorInfo: ErrorInfo) {
        console.error('Uncaught error:', error, errorInfo);
    }

    public render() {
        if (this.state.hasError) {
            return (
                this.props.fallback || (
                    <div className="min-h-screen flex items-center justify-center p-4 bg-background">
                        <div className="max-w-md w-full bg-card border border-border/10 rounded-lg p-8 text-center shadow-lg">
                            <AlertCircle className="h-12 w-12 text-destructive mx-auto mb-4" />
                            <h1 className="text-xl font-semibold mb-2 text-foreground">Something went wrong</h1>
                            <p className="text-muted-foreground mb-6">
                                An unexpected error occurred.
                            </p>
                            <div className="bg-card/60 border border-destructive/20 rounded-lg p-4 mb-6 text-left relative">
                                {/* Error badge */}
                                <div className="absolute -top-3 left-4 px-2 py-0.5 bg-destructive-background text-destructive text-xs font-medium rounded-full border border-destructive/20">
                                    Error Details
                                </div>

                                {/* Styled error message */}
                                <div className="mt-2">
                                    <pre className="text-sm font-mono whitespace-pre-wrap overflow-auto max-h-[200px] p-2 rounded bg-card/40 text-foreground/90">
                                        {this.state.error?.toString()}
                                    </pre>
                                </div>

                                {/* Stack trace if available */}
                                {this.state.errorInfo?.componentStack && (
                                    <details className="mt-3">
                                        <summary className="text-xs text-muted-foreground hover:text-foreground cursor-pointer">
                                            Stack Trace
                                        </summary>
                                        <pre className="mt-2 text-xs font-mono whitespace-pre-wrap overflow-auto max-h-[150px] p-2 rounded bg-card/40 text-foreground/70">
                                            {this.state.errorInfo.componentStack}
                                        </pre>
                                    </details>
                                )}
                            </div>
                            <button
                                onClick={() => window.location.href = '/'}
                                className="flex items-center gap-2 bg-primary text-primary-foreground px-4 py-2 rounded-lg 
                                         hover:bg-primary/90 transition-colors mx-auto"
                            >
                                <Home size={16} />
                                Go Home
                            </button>
                        </div>
                    </div>
                )
            );
        }

        return this.props.children;
    }
}