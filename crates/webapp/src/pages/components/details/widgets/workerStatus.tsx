import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Clock } from 'lucide-react'

export function WorkerStatus({ totalWorkers }: { totalWorkers: number }) {
  return (
    <Card>
      <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
        <CardTitle className="text-base font-medium">Worker Status</CardTitle>
        <Clock className="h-4 w-4 text-muted-foreground" />
      </CardHeader>
      <CardContent className="pt-4">
        <div className="relative h-[300px] w-[300px] mx-auto">
          <svg className="h-full w-full" viewBox="0 0 100 100">
            <circle
              cx="50"
              cy="50"
              r="40"
              fill="none"
              stroke="currentColor"
              strokeWidth="20"
              className="text-green-500"
            />
            <circle
              cx="50"
              cy="50"
              r="20"
              fill="white"
              className="text-background"
            />
          </svg>
          <div className="absolute inset-0 flex items-center justify-center flex-col">
            <span className="text-4xl font-bold">{totalWorkers}</span>
            <span className="text-sm text-muted-foreground">Total Workers</span>
          </div>
        </div>
      </CardContent>
    </Card>
  )
}

