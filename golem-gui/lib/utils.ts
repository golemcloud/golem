/* eslint-disable @typescript-eslint/no-unused-vars */
import { GolemError } from "@/types/api";
import { clsx, type ClassValue } from "clsx"
import { twMerge } from "tailwind-merge"
import { BACKEND_URL } from "@/lib/config";


export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}

export const getErrorMessage = (error: GolemError | string): string => {

  if(typeof error === "string") {
    return error;
  }

  if (error.golemError) {
    return `${error.golemError.type}: ${error.golemError.details}`;
  }

  if (error.errors?.length) {
    return error.errors.join(', ');
  }

  if (error.error) {
    return error.error;
  }

  return 'An unknown error occurred';
};

export const fetcher = (url:string, options?:RequestInit) => fetch(`/api/proxy${url}`, options ).then((res) => res.json());


