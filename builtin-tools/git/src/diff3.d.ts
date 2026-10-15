declare module "diff3" {
  export default function merge(
    ours: string[],
    base: string[],
    theirs: string[],
  ): (
    | { ok: string[]; conflict?: never }
    | { ok?: never; conflict: { a: string[]; o: string[]; b: string[] } }
  )[];
}
