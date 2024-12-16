'use client'
import Link from 'next/link';
import { Box, Typography } from '@mui/material';
import { useRouter } from 'next/navigation';
import { useEffect } from 'react';


export default function Home() {
  const router = useRouter();
  useEffect(()=>{
    router.push('/overview');
  }, [router])
  return (
    <Box sx={{ textAlign: 'center', padding: '2rem' }}>
      <Typography variant="h4" gutterBottom>
        Welcome to Golem UI
      </Typography>
      <Typography variant="h6" gutterBottom>
        Navigate to:
      </Typography>
      <Box>
        <Link href="/components">Component Management</Link> |{' '}
        <Link href="/workers">Worker Management</Link> |{' '}
        <Link href="/apis">API Management</Link> |{' '}
        <Link href="/plugins">Plugin Management</Link>
      </Box>
    </Box>
  );
}
