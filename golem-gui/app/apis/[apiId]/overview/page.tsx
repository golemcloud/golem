'use client'
import RoutePage from '@/components/api-route-page';
import DeploymentPage from '@/components/deployment';
import { Box, Typography, Paper } from '@mui/material';
import { useParams } from 'next/navigation';

export default function Overview() {
  const { apiId } = useParams<{ apiId: string }>();
  return (
    <Box>
      {/* Routes Section */}
      <Paper
        elevation={3}
         className="bg-[#333]"
        sx={{
          p: 3,
          mb: 3,
          color: 'text.primary',
          border: 1,
          borderColor: 'divider',
          borderRadius: 2,
        }}
      >
        <Typography variant="h6" gutterBottom>
          Routes
        </Typography>
        <RoutePage apiId={apiId} limit={5}/>
      </Paper>

      {/* Active Deployments Section */}
        <DeploymentPage apiId={apiId} limit={5}/>
    </Box>
  );
}
