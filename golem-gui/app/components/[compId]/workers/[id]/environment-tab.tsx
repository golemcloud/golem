import { Worker } from '@/types/api'
import { Paper, Typography, Divider, Stack, Box, List } from '@mui/material';
import {  Code2 } from 'lucide-react';
import React from 'react'

export default function EnvironmentTab({worker}:{worker:Worker}) {

  const envs = Object.keys(worker?.env || {});
  return (
    <div className="mx-auto max-w-7xl px-2 md:px-6 lg:px-8">
      <div className="mx-auto max-w-2xl lg:max-w-none py-4">
        <Paper
          elevation={3}
          sx={{
            p: 3,
            mb: 3,
            borderRadius: 2,
          }}
          className="border"
        >
          <Stack direction="row" alignItems={"center"} justifyContent={"space-between"}>
          <Typography variant="subtitle1">Environment</Typography>
            <Code2/>
            </Stack>  
          <Divider className="my-2 bg-border" />
          <Box>
        <List>
          {envs?.map((env , index: number) => (
            <Stack key={env}>
              {index > 0 && <Divider className="my-1 bg-border"/>}
               <Stack direction="row" alignItems={"center"} justifyContent={"space-between"} my={2}>
                    <Typography>{env}</Typography>
                    {/* TODO this can be improve like gloem */}
                    <input type="password" value="***********" className='border-none cursor-default' disabled/>
                </Stack> 
            </Stack>
          ))}
        </List>
    </Box>
        </Paper>
      </div>
    </div>
  )
}
