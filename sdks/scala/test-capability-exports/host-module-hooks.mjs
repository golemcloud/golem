const sources = new Map();

export function initialize({ modules }) {
  for (const [specifier, source] of modules) {
    sources.set(specifier, source);
  }
}

export async function resolve(specifier, context, nextResolve) {
  if (sources.has(specifier)) {
    return {
      url: `data:text/javascript,${encodeURIComponent(sources.get(specifier))}`,
      shortCircuit: true,
    };
  }
  return nextResolve(specifier, context);
}
