import React from 'react';
import { Box, Typography, Grid, Paper,Divider } from '@mui/material';
import InsertDriveFileOutlinedIcon from '@mui/icons-material/InsertDriveFileOutlined';
import ViewModuleOutlinedIcon from '@mui/icons-material/ViewModuleOutlined';
import LanguageOutlinedIcon from '@mui/icons-material/LanguageOutlined';
import BuildOutlinedIcon from '@mui/icons-material/BuildOutlined';

const OverviewFooter = () => {
  return (
    <Box
      sx={{
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        marginY: 4,
      }}
    >
      <Paper
        elevation={0}
        sx={{
          p: 4,
          borderRadius: 2,
          maxWidth: '100%',
          overflow: 'auto',
        }}
      >
        <Grid container spacing={3} justifyContent="space-between" alignItems="center" wrap="nowrap">
          {/* Language Guides */}
          <Grid item>
            <Box display="flex" alignItems="flex-start">
              <InsertDriveFileOutlinedIcon sx={{fontSize: 32, mr: 2}} />
              <Box>
                <Typography variant="h6" fontWeight="bold">
                  Language Guides
                </Typography>
                <Typography variant="body2" color="gray">
                  Choose your language and start building
                </Typography>
              </Box>
            </Box>
          </Grid>

          {/* Components */}
          <Grid item>
            <Box display="flex" alignItems="flex-start">
              <ViewModuleOutlinedIcon sx={{fontSize: 32, mr: 2 }} />
              <Box>
                <Typography variant="h6" fontWeight="bold">
                  Components
                </Typography>
                <Typography variant="body2" color="gray">
                  Create WASM components that run on Golem
                </Typography>
              </Box>
            </Box>
          </Grid>

          {/* APIs */}
          <Grid item>
            <Box display="flex" alignItems="flex-start">
              <LanguageOutlinedIcon sx={{fontSize: 32, mr: 2 }} />
              <Box>
                <Typography variant="h6" fontWeight="bold">
                  APIs
                </Typography>
                <Typography variant="body2" color="gray">
                  Craft custom APIs to expose your components to the world
                </Typography>
              </Box>
            </Box>
          </Grid>

          {/* Workers */}
          <Grid item>
            <Box display="flex" alignItems="flex-start">
              <BuildOutlinedIcon sx={{fontSize: 32, mr: 2 }} />
              <Box>
                <Typography variant="h6" fontWeight="bold">
                  Workers
                </Typography>
                <Typography variant="body2" color="gray">
                  Launch and manage efficient workers from your components
                </Typography>
              </Box>
            </Box>
          </Grid>
        </Grid>
      </Paper>
    </Box>
  );
};

export default OverviewFooter;
