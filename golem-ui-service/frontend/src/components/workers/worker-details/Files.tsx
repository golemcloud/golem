import {
    Download,
    File,
    FileText,
    Folder,
    FolderOpen,
    Loader2,
    Lock,
    RefreshCcw,
    UnlockKeyhole
} from 'lucide-react';
import { WorkerFile, downloadWorkerFile, useWorkerFiles } from '../../../api/workers';

import React from 'react';
import { Worker, } from '../../../types/api';
import { displayError } from '../../../lib/error-utils';
import toast from 'react-hot-toast';
import { useQueryClient } from '@tanstack/react-query';

interface FilesTabProps {
    worker: Worker;
}

const FileRow: React.FC<{
    file: WorkerFile;
    onDownload: (fileName: string) => void;
}> = ({ file, onDownload }) => {
    const formatFileSize = (bytes: number) => {
        if (bytes === 0) return '0 Bytes';
        const k = 1024;
        const sizes = ['Bytes', 'KB', 'MB', 'GB'];
        const i = Math.floor(Math.log(bytes) / Math.log(k));
        return `${parseFloat((bytes / Math.pow(k, i)).toFixed(2))} ${sizes[i]}`;
    };

    const formatDate = (timestamp: number) => {
        return new Date(timestamp).toLocaleString();
    };

    return (
        <div className="p-4 bg-card/60 rounded-lg hover:bg-card/70 transition-colors group">
            <div className="flex items-center justify-between">
                <div className="flex items-center gap-3">
                    {file.kind === 'directory' ? (
                        <FolderOpen size={16} className="text-blue-400" />
                    ) : (
                        <FileText size={16} className="text-primary" />
                    )}
                    <div>
                        <div className="font-medium flex items-center gap-2">
                            {file.name}
                            {file.permissions === 'read-only' ? (
                                <Lock size={12} className="text-muted-foreground" />
                            ) : (
                                <UnlockKeyhole size={12} className="text-muted-foreground" />
                            )}
                        </div>
                        <div className="text-sm text-muted-foreground">
                            {formatFileSize(file.size)} • Last modified {formatDate(file.lastModified)}
                        </div>
                    </div>
                </div>
                {file.kind === 'file' && (
                    <button
                        onClick={() => onDownload(file.name)}
                        className="p-2 text-primary hover:text-primary-accent rounded-md hover:bg-card/60 opacity-0 group-hover:opacity-100 transition-opacity"
                        title="Download file"
                    >
                        <Download size={16} />
                    </button>
                )}
            </div>
        </div>
    );
};

const FilesTab: React.FC<FilesTabProps> = ({ worker }) => {
    const queryClient = useQueryClient();
    const { data: filesData, isLoading, error } = useWorkerFiles(
        worker.workerId.componentId,
        worker.workerId.workerName
    );

    const handleDownload = async (fileName: string) => {
        try {
            const blob = await downloadWorkerFile(
                worker.workerId.componentId,
                worker.workerId.workerName,
                fileName
            );

            // Create download link
            const url = window.URL.createObjectURL(blob);
            const a = document.createElement('a');
            a.href = url;
            a.download = fileName;
            document.body.appendChild(a);
            a.click();
            window.URL.revokeObjectURL(url);
            document.body.removeChild(a);

            toast.success('File download started');
        } catch (error) {
            displayError(error, 'Failed to download file');
            console.error('Download failed:', error);
        }
    };

    const handleRefresh = () => {
        queryClient.invalidateQueries({
            queryKey: ['workers', worker.workerId.componentId, worker.workerId.workerName, 'files']
        });
    };

    if (error) {
        return (
            <div className="bg-card/80 border border-border/10 rounded-lg p-6">
                <div className="text-center py-8 text-destructive">
                    <File size={24} className="mx-auto mb-2 opacity-50" />
                    <p>Failed to load files</p>
                    <button
                        onClick={handleRefresh}
                        className="mt-2 text-sm text-primary hover:text-primary-accent"
                    >
                        Try again
                    </button>
                </div>
            </div>
        );
    }

    return (
        <div className="bg-card/80 border border-border/10 rounded-lg p-6">
            <div className="flex items-center justify-between mb-4">
                <h3 className="text-lg font-semibold flex items-center gap-2">
                    <Folder size={20} className="text-primary" />
                    Worker Files
                </h3>
                <button
                    className="p-2 text-muted-foreground hover:text-foreground rounded-lg hover:bg-card/60 transition-colors"
                    onClick={handleRefresh}
                    disabled={isLoading}
                >
                    <RefreshCcw size={16} className={isLoading ? 'animate-spin' : ''} />
                </button>
            </div>

            <div className="space-y-2">
                {isLoading ? (
                    <div className="flex items-center justify-center py-8 text-muted-foreground">
                        <Loader2 size={24} className="animate-spin" />
                    </div>
                ) : filesData?.nodes && filesData.nodes.length > 0 ? (
                    filesData.nodes.map((file: WorkerFile) => (
                        <FileRow
                            key={file.name}
                            file={file}
                            onDownload={handleDownload}
                        />
                    ))
                ) : (
                    <div className="text-center py-8 text-muted-foreground">
                        <Folder size={24} className="mx-auto mb-2 opacity-50" />
                        <p>No files available</p>
                    </div>
                )}
            </div>
        </div>
    );
};

export default FilesTab;