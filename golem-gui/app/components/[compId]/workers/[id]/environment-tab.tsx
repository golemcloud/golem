import { Worker } from '@/types/api'
import { Paper, Typography, Divider, Stack, Box, List } from '@mui/material';
import { Code2, Eye, EyeClosed } from 'lucide-react';
import ContentCopyIcon from '@mui/icons-material/ContentCopy';
import React, { useState } from 'react'

export default function EnvironmentTab({worker}:{worker:Worker}) {

  const [show, setShow] = useState<Record<string,boolean>>({});
  const handleCopyToClipboard = (textToCopy:string) => {
    navigator.clipboard
      .writeText(textToCopy)
      .then(() => {
        alert('Copied to clipboard!');
      })
      .catch((err) => {
        console.error('Failed to copy text: ', err);
      });
  };

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
        {envs?.length >0 ?<List>
          {envs?.map((env , index: number) => (
            <Stack key={env}>
              {index > 0 && <Divider className="my-1 bg-border"/>}
               <Stack direction="row" alignItems={"center"} justifyContent={"space-between"} my={2}>
                    <Typography>{env}</Typography>
                    {/* TODO this can be improve like gloem */}
                    <Stack direction="row" gap={1} alignItems={"center"}>
                      <Typography 
                      sx={{
                        fontWeight: show && show[env] ? 'normal' : 'bold',
                        letterSpacing: show && show[env] ? 'inherit' : '3px',
                        pt: show && show[env] ?  0 : 1 
                      }}
                      
                      >{show && show[env]? env: "******************"}</Typography>
                      <ContentCopyIcon onClick={()=>handleCopyToClipboard(env)}/>
                      <Box 
                      onClick={(e)=>{
                        e.preventDefault();
                        if(show && show[env]){
                          return setShow((prev)=>{
                            const newShow = {...prev}
                            if(newShow){
                              delete newShow[env]
                            }
                            return newShow;
                          })
                        }
                        setShow((prev)=>({...(prev||{}), [env]: true}))}}
                        >{show?.[env]? <EyeClosed/>: <Eye/>}
                    </Box>
                    </Stack> 
                </Stack> 
            </Stack>
          ))}
        </List> : <Typography variant="body2" sx={{ color: "#AAAAAA", textAlign:"center" }}>No environment variables</Typography>}
    </Box>
        </Paper>
      </div>
    </div>
  )
}
