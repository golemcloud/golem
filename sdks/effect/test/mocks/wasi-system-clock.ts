export const now = () => {
  const milliseconds = Date.now()
  return {
    seconds: BigInt(Math.floor(milliseconds / 1000)),
    nanoseconds: (milliseconds % 1000) * 1_000_000,
  }
}
