"use client";

import React from "react";
import { useForm, Controller } from "react-hook-form";
import {
  Box,
  Button,
  TextField,
  Typography,
  Select,
  MenuItem,
  InputLabel,
  FormControl,
  Divider,
} from "@mui/material";
import useSWR from "swr";
import { fetcher } from "@/lib/utils";
import { Component } from "@/types/api";
import { useParams, useSearchParams } from "next/navigation";
import NewRouteForm from "@/components/new-route";

export default function Page() {
  const {apiId} =useParams<{apiId:string}>()
  const searchParams = useSearchParams(); 
  return <NewRouteForm apiId={apiId} version={searchParams?.get("version") || ""}/>
}