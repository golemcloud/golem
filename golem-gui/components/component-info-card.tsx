import React from 'react';
import { CardContent, Typography, Divider,Paper } from '@mui/material';

interface ComponentInfoProps {
  componentId: string;
  version: string | number;
  name: string;
  size: string | number;
  createdAt: string;
}

const ComponentInfo: React.FC<ComponentInfoProps> = ({ componentId, version, name, size, createdAt }) => {
  return (
    <Paper sx={{width:"100%" }} elevation={4}>
      <CardContent>
        <Typography variant="body2" gutterBottom>
          <strong>Component ID</strong>
        </Typography>
        <Typography variant="body2" color="text.secondary" sx={{ color: '#aaa' }}>
          {componentId}
        </Typography>

        <Divider sx={{ my: 1, bgcolor: '#444' }} />

        <Typography variant="body2" gutterBottom>
          <strong>Version</strong>
        </Typography>
        <Typography variant="body2" color="text.secondary" sx={{ color: '#aaa' }}>
          {version}
        </Typography>

        <Divider sx={{ my: 1, bgcolor: '#444' }} />

        <Typography variant="body2" gutterBottom>
          <strong>Name</strong>
        </Typography>
        <Typography variant="body2" color="text.secondary" sx={{ color: '#aaa' }}>
          {name}
        </Typography>

        <Divider sx={{ my: 1, bgcolor: '#444' }} />

        <Typography variant="body2" gutterBottom>
          <strong>Size</strong>
        </Typography>
        <Typography variant="body2" color="text.secondary" sx={{ color: '#aaa' }}>
          {size}
        </Typography>

        <Divider sx={{ my: 1, bgcolor: '#444' }} />

        <Typography variant="body2" gutterBottom>
          <strong>Created At</strong>
        </Typography>
        <Typography variant="body2" color="text.secondary" sx={{ color: '#aaa' }}>
          {createdAt}
        </Typography>
      </CardContent>
    </Paper>
  );
};

export default ComponentInfo;
