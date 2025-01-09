import React, { useState } from 'react';
import { Stack, Typography, Box } from '@mui/material';
import ContentCopyIcon from '@mui/icons-material/ContentCopy';
import Eye from '@mui/icons-material/Visibility';
import EyeClosed from '@mui/icons-material/VisibilityOff';

const ClipboardVisibilityToggle = ({ value, maskedText = "******************" }:{value:string, maskedText?:string}) => {
  const [show, setShow] = useState(false);

  const handleCopyToClipboard = () => {
    navigator.clipboard
      .writeText(value)
      .then(() => {
        alert('Copied to clipboard!');
      })
      .catch((err) => {
        console.error('Failed to copy text:', err);
      });
  };

  const toggleVisibility = () => {
    setShow((prevShow) => !prevShow);
  };

  return (
    <Stack direction="row" gap={1} alignItems="center">
      <Typography
        sx={{
          fontWeight: show ? 'normal' : 'bold',
          letterSpacing: show ? 'inherit' : '3px',
          pt: show ? 0 : 1,
        }}
      >
        {show ? value : maskedText}
      </Typography>
      <ContentCopyIcon 
        onClick={handleCopyToClipboard} 
        sx={{ cursor: 'pointer' }}
      />
      <Box 
        onClick={(e) => {
          e.preventDefault();
          toggleVisibility();
        }}
        sx={{ cursor: 'pointer' }}
      >
        {show ? <EyeClosed /> : <Eye />}
      </Box>
    </Stack>
  );
};

export default ClipboardVisibilityToggle;
