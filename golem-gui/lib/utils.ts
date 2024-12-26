/* eslint-disable @typescript-eslint/no-unused-vars */
import { GolemError } from "@/types/api";
import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";
import { BACKEND_URL } from "@/lib/config";
import { FieldErrors } from "react-hook-form";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

export const getErrorMessage = (error: GolemError | string): string => {
  if (typeof error === "string") {
    return error;
  }

  if (error.golemError) {
    return `${error.golemError.type}: ${error.golemError.details}`;
  } 

  if (error.errors?.length) {
    return error.errors.join(", ");
  }

  if (error.error) {
    return error.error;
  }

  return "An unknown error occurred";
};

export function calculateHoursDifference(createdAt: string): string {
  const createdAtDate = new Date(createdAt);
  const currentDate = new Date();
  const differenceInMs = currentDate.getTime() - createdAtDate.getTime();
  const differenceInHours = Math.round(differenceInMs / (1000 * 60 * 60));
  if (differenceInHours >= 24) {
    return `${Math.round(differenceInHours / 24)} days ago`;
  }
  return `${differenceInHours} hours ago`;
}

export function calculateSizeInMB(sizeInBytes: number): string {
  return (sizeInBytes / (1024 * 1024)).toFixed(2);
}

export const fetcher = (url: string, options?: RequestInit) =>
  fetch(`/api/proxy${url}`, options).then((res) => res.json());


//we can replace with lodash.
export function getFormErrorMessage<T extends Record<string, unknown>>(
  key: string,
  errors: FieldErrors<T>
): string | undefined {
  const pathSegments = key.split(".");
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  let current: any = errors;

  for (const segment of pathSegments) {
    const match = segment.match(/^\[(\d+)]$/);
    if (match) {
      const index = Number(match[1]);
      if (Array.isArray(current)) {
        current = current[index];
      } else {
        return undefined;
      }
    } else {
      if (typeof current === "object" && current !== null) {
        current = current[segment as keyof typeof current];
      } else {
        return undefined;
      }
    }
  }

  return current?.message;
}

