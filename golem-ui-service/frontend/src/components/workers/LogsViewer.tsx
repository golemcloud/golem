import { ChevronLeft, ChevronRight, Code2, Terminal } from 'lucide-react';

import { useState } from 'react';

const LogEntry = ({ entry }) => {
    const getLogContent = () => {
        if (entry.type === 'Log') {
            return entry.message;
        } else if (entry.type === 'ExportedFunctionInvoked') {
            return entry.function_name;
        }
        return '';
    };

    const getLogColor = () => {
        if (entry.type === 'Log') {
            return 'text-success';
        } else if (entry.type === 'ExportedFunctionInvoked') {
            return 'text-primary';
        }
        return 'text-muted-foreground';
    };

    return (
        <div className="p-3 bg-card/50 hover:bg-card/70 border border-border/10 rounded-lg transition-all">
            <div className="flex items-center justify-between">
                <div className="flex items-center gap-2">
                    {entry.type === 'Log' ? (
                        <Terminal size={14} className={getLogColor()} />
                    ) : (
                        <Code2 size={14} className={getLogColor()} />
                    )}
                    <span className="text-sm text-muted-foreground">
                        {new Date(entry.timestamp).toLocaleString()}
                    </span>
                </div>
                {entry.type === 'Log' && (
                    <span className={`text-xs px-2 py-1 rounded-full ${entry.type === 'Log' ? 'bg-success-background text-success' : 'bg-primary-background text-primary'}`}>
                        {entry.type}
                    </span>
                )}
            </div>
            <div className={`text-sm font-mono text-foreground/90 mt-2 text-left ${entry.type === 'Log' ? 'whitespace-pre-wrap' : ''}`}>
                {getLogContent()}
            </div>
        </div>
    );
};

const LogsViewer = ({ logs }) => {
    const [currentPage, setCurrentPage] = useState(1);
    const [activeTab, setActiveTab] = useState('terminal');
    const itemsPerPage = 20;

    const filteredLogs = logs.entries.filter(({ entry }) => {
        if (activeTab === 'terminal') return entry.type === 'Log';
        if (activeTab === 'invocations') return entry.type === 'ExportedFunctionInvoked';
        return false;
    });

    const totalPages = Math.ceil(filteredLogs.length / itemsPerPage);
    const startIndex = (currentPage - 1) * itemsPerPage;
    const endIndex = startIndex + itemsPerPage;
    const currentLogs = filteredLogs.slice(startIndex, endIndex);

    return (
        <div className="space-y-4">
            {/* Sticky Tabs */}
            <div
                className="flex gap-2 bg-card/90 border-b border-border/10 backdrop-blur-sm sticky top-0 z-10 p-2"
            >
                {[
                    { id: 'terminal', label: 'Terminal' },
                    { id: 'invocations', label: 'Invocations' }
                ].map((tab) => (
                    <button
                        key={tab.id}
                        onClick={() => {
                            setActiveTab(tab.id);
                            setCurrentPage(1);
                        }}
                        className={`px-4 py-2 rounded-lg transition-colors ${
                            activeTab === tab.id
                                ? 'bg-primary text-primary-foreground'
                                : 'text-muted-foreground hover:text-foreground hover:bg-card/60'
                        }`}
                    >
                        {tab.label}
                    </button>
                ))}
            </div>

            {/* Logs */}
            <div className="space-y-2 min-h-[32rem] overflow-y-auto">
                {currentLogs.reverse().map(({ entry }, index) => (
                    <LogEntry key={index} entry={entry} />
                ))}
                {currentLogs.length === 0 && (
                    <div className="text-center py-8 text-muted-foreground">
                        <Terminal className="w-8 h-8 mx-auto mb-2 opacity-50" />
                        <p>No logs to display</p>
                        <p className="text-sm mt-1">Logs will appear here when they are generated</p>
                    </div>
                )}
            </div>

            {/* Pagination */}
            {totalPages > 1 && (
                <div className="flex items-center justify-between border-t border-border/10 pt-4">
                    <div className="flex items-center gap-2">
                        <span className="text-sm text-muted-foreground">
                            Page {currentPage} of {totalPages}
                        </span>
                    </div>
                    <div className="flex items-center gap-2">
                        <button
                            onClick={() => setCurrentPage((prev) => Math.max(1, prev - 1))}
                            disabled={currentPage === 1}
                            className="p-2 text-muted-foreground hover:text-foreground rounded-lg hover:bg-card/60 
                                     transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                        >
                            <ChevronLeft size={16} />
                        </button>
                        <button
                            onClick={() => setCurrentPage((prev) => Math.min(totalPages, prev + 1))}
                            disabled={currentPage === totalPages}
                            className="p-2 text-muted-foreground hover:text-foreground rounded-lg hover:bg-card/60 
                                     transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                        >
                            <ChevronRight size={16} />
                        </button>
                    </div>
                </div>
            )}
        </div>
    );
};

export default LogsViewer;