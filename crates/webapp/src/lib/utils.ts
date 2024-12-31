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


