import { clsx, type ClassValue } from "clsx"
import { twMerge } from "tailwind-merge"

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}

export function formatRelativeTime(dateString: string | number | Date) {
  const date = new Date(dateString).getTime();
  const now = new Date().getTime();
  const diffInSeconds = Math.floor((now - date) / 1000);

  const units = [
    { name: 'year', seconds: 60 * 60 * 24 * 365 },
    { name: 'month', seconds: 60 * 60 * 24 * 30 },
    { name: 'week', seconds: 60 * 60 * 24 * 7 },
    { name: 'day', seconds: 60 * 60 * 24 },
    { name: 'hour', seconds: 60 * 60 },
    { name: 'minute', seconds: 60 },
    { name: 'second', seconds: 1 },
  ];

  for (const unit of units) {
    if (diffInSeconds >= unit.seconds) {
      const value = Math.floor(diffInSeconds / unit.seconds);
      return `${value} ${unit.name}${value > 1 ? 's' : ''} ago`;
    }
  }

  return 'just now';
}

export const sanitizeInput = (input: string): string => {
  return input
    .replace(/\u201c|\u201d/g, '"') 
    .replace(/'/g, '"'); 
};

export function formatTimestampInDateTimeFormat(timestamp: string) {
  const date = new Date(timestamp);

  // Get date components
  const month = String(date.getMonth() + 1).padStart(2, "0"); // Months are zero-indexed
  const day = String(date.getDate()).padStart(2, "0");

  // Get time components
  const hours = String(date.getHours()).padStart(2, "0");
  const minutes = String(date.getMinutes()).padStart(2, "0");
  const seconds = String(date.getSeconds()).padStart(2, "0");
  const milliseconds = String(date.getMilliseconds()).padStart(3, "0");

  // Combine into the desired format
  return `${month}-${day} ${hours}:${minutes}:${seconds}.${milliseconds}`;
}




