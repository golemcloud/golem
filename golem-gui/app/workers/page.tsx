import { Box, Typography } from '@mui/material';

export default function WorkersPage() {
  return (
    <Box sx={{ padding: '2rem' }}>
      <Typography variant="h4" gutterBottom>
        Worker Management
      </Typography>
      <Typography variant="body1">Manage your workers here.</Typography>
    </Box>
  );
}
