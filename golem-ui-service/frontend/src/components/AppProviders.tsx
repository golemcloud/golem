import { BrowserRouter } from 'react-router-dom';
import { ErrorBoundary } from './ErrorBoundary';
import { QueryProvider } from '../providers/query-provider';
import { ReactNode } from 'react';

interface AppProvidersProps {
    children: ReactNode;
}

export const AppProviders = ({ children }: AppProvidersProps) => {
    return (
        <ErrorBoundary>
            <QueryProvider>
                <BrowserRouter>
                    {children}
                </BrowserRouter>
            </QueryProvider>
        </ErrorBoundary>
    );
};