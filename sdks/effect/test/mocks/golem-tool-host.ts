/** Tool tests inject ToolClient or a custom transport rather than call ambient bindings. */
export function getAllTools(): never {
  throw new Error("provide a ToolClient test layer")
}
export const getTool = getAllTools
export const createStdin = getAllTools
export const createStdinFromStream = getAllTools
export const createStdout = getAllTools
export class ToolRpc {
  constructor() {
    getAllTools()
  }
}
