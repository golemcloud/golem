import { Pause, Play, Shield, Trash } from 'lucide-react';

import React from 'react';
import { Worker } from '../../../types/api';

interface AdvancedTabProps {
  worker: Worker;
  onAction: (action: 'interrupt' | 'resume' | 'delete') => void;
}

const AdvancedTab: React.FC<AdvancedTabProps> = ({ worker, onAction }) => {
  return (
    <div className="space-y-6">
      <div className="bg-card/80 border border-border/10 rounded-lg p-6">
        <h3 className="text-lg font-semibold flex items-center gap-2 mb-4">
          <Shield size={20} className="text-primary" />
          Advanced Settings
        </h3>
        <div className="space-y-4">
          <button
            onClick={() => onAction('interrupt')}
            // disabled={!['Running'].includes(worker.status)}
            className="w-full flex items-center justify-between p-4 bg-card/60 rounded-lg hover:bg-card/70 transition-colors disabled:opacity-50"
          >
            <div className="flex items-center gap-3">
              <Pause size={16} className="text-destructive" />
              <div className="text-left">
                <div className="font-medium">Interrupt Worker</div>
                <div className="text-sm text-muted-foreground">
                  Temporarily pause worker execution
                </div>
              </div>
            </div>
          </button>

          <button
            onClick={() => onAction('resume')}
            // disabled={!['Interrupted', 'Failed', 'Exited'].includes(worker.status)}
            className="w-full flex items-center justify-between p-4 bg-card/60 rounded-lg hover:bg-card/70 transition-colors disabled:opacity-50"
          >
            <div className="flex items-center gap-3">
              <Play size={16} className="text-success" />
              <div className="text-left">
                <div className="font-medium">Resume Worker</div>
                <div className="text-sm text-muted-foreground">
                  Resume worker execution
                </div>
              </div>
            </div>
          </button>

          <button
            onClick={() => {
              if (confirm("Are you sure you want to delete this worker? This action cannot be undone.")) {
                onAction('delete');
              }
            }}
            className="w-full flex items-center justify-between p-4 bg-destructive/10 rounded-lg hover:bg-destructive/20 transition-colors"
          >
            <div className="flex items-center gap-3">
              <Trash size={16} className="text-destructive" />
              <div className="text-left">
                <div className="font-medium">Delete Worker</div>
                <div className="text-sm text-muted-foreground">
                  Permanently delete this worker
                </div>
              </div>
            </div>
          </button>
        </div>
      </div>
    </div>
  );
};

export default AdvancedTab;