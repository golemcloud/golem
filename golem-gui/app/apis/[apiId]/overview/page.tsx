import { Box, Typography, Paper, Button } from '@mui/material';

export default function Overview() {
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
        <Typography variant="body2">
          No routes defined for this API version.
        </Typography>
      </Paper>

      {/* Active Deployments Section */}
      <Paper
        elevation={3}
         className="bg-[#333]"
        sx={{
          p: 3,
          bgcolor: 'background.paper',
          color: 'text.primary',
          border: 1,
          borderColor: 'divider',
          borderRadius: 2,
        }}
      >
        <Box
          display="flex"
          justifyContent="space-between"
          alignItems="center"
          mb={1}
        >
          <Typography variant="h6">Active Deployments</Typography>
          <Button variant="contained" color="primary" size="small">
            View All
          </Button>
        </Box>
        <Typography variant="body2" >
          No active deployments for this API version.
        </Typography>
      </Paper>
    </Box>
  );
}
