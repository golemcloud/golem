import React from 'react';
import { Box, AppBar, Toolbar, Tabs, Tab, Button, Divider, Typography } from '@mui/material';

const TerminalPage = ({ workerName }: { workerName: string }) => {
  const [activeTab, setActiveTab] = React.useState(0);

  const handleTabChange = (event: React.SyntheticEvent, newValue: number) => {
    setActiveTab(newValue);
  };

  return (
    <Box className="text-black dark:text-white" sx={{ width: '100%', height: '100vh' }}>
      <Divider className="border-gray-300 dark:border-gray-700" sx={{ marginTop: '4px' }} />

      <AppBar
        position="static"
        className="bg-gray-200 dark:bg-[#333] border-b border-gray-300 dark:border-gray-700"
      >
        <Toolbar
          sx={{ justifyContent: 'center', borderBottom: '1px solid #333' }}
          className="dark:border-gray-600"
        >
          <Typography
            variant="h6"
            sx={{ fontWeight: 'bold' }}
            className="text-gray-700 dark:text-gray-300"
          >
            {workerName}
          </Typography>
        </Toolbar>

        <Toolbar>
          <Tabs
            value={activeTab}
            indicatorColor="primary"
            onChange={handleTabChange}
            sx={{ flexGrow: 1 }}
            className="text-gray-700 dark:text-gray-300"
          >
            <Tab label="Terminal" className="text-gray-700 dark:text-gray-300" />
            <Tab label="Invocations" className="text-gray-700 dark:text-gray-300" />
            <Tab label="Logs" className="text-gray-700 dark:text-gray-300" />
          </Tabs>
          <Button
            variant="outlined"
            color="error"
            sx={{ marginRight: 1 }}
            className="dark:border-red-600"
          >
            Clear
          </Button>
          <Button variant="contained" color="primary">
            Reload
          </Button>
        </Toolbar>
      </AppBar>

      <Box sx={{ flex: 1, p: 2, overflowY: 'auto' }} className="text-gray-700 dark:text-gray-300">
        {activeTab === 0 && <Box className="text-center">Terminal output...</Box>}
        {activeTab === 1 && <Box className="text-center">Invocation data...</Box>}
        {activeTab === 2 && <Box className="text-center">Logs output...</Box>}
      </Box>
    </Box>
  );
};

export default TerminalPage;
