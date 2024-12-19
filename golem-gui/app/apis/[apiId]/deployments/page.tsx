"use client";

import { Box, Typography, Paper, Button } from '@mui/material';

export default function ApiStatus() {
  return (
    <Box>
      {/* Active Deployments Section */}
      <Paper
       className="bg-[#333]"
        elevation={3}
        sx={{
          p: 3,
          mb: 3,
          color: 'text.primary',
          backgroundColor: '#333', // Use this for the background color
          border: 1,
          borderColor: 'divider',
          borderRadius: 2,
        }}
      >
        <Box
       
         sx={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', mb: 2 }}>
          <Typography variant="h6">Active Deployments</Typography>
          <Button variant="contained" color="primary" onClick={() => console.log("Button clicked")}>
            View All
          </Button>
        </Box>
        <Typography variant="body2">
          No routes defined for this API version.
        </Typography>
      </Paper>
    </Box>
  );
}
