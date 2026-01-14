export function validateIdentifier(name: string) {
  if (!/^[a-zA-Z][a-zA-Z0-9_]*$/.test(name)) {
    throw new Error(`Invalid variable name "${name}"`);
  }
}
