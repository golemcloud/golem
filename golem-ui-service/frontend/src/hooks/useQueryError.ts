import { UseQueryResult } from '@tanstack/react-query'

interface ApiError {
  error?: string
  errors?: string[]
  type?: string
  golemError?: {
    type: string
    details: string
  }
}

export const useQueryError = <T>(query: UseQueryResult<T, Error | ApiError>) => {
  if (!query.error) return null

  if (query.error instanceof Error) {
    return query.error.message
  }

  const error = query.error as ApiError

  if (error.golemError) {
    return `${error.golemError.type}: ${error.golemError.details}`
  }

  if (error.errors?.length) {
    return error.errors.join(', ')
  }

  if (error.error) {
    return error.error
  }

  return 'An unknown error occurred'
}