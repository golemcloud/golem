# Problems that could be solved in the MoonBit ecosystem

- The `mbti` parser is not up-to-date, had to parse `mbt` for reexports
- The need to generate reexports
- `formatter` package not up-to-date
- Could not find a package to parse/print the new `pkg` format 
- mbti files in the source tree are noise
- another `pre-build` hook that runs earlier so it can alter `pkg` files (needed for reexports link section generation)
- `post-build` hook
- having to build a cli tool for 'derivation'
- hard to change and regenerate bindings with wit-bindgen moonbit. many leftovers 
- wit-bindgen still generating pkg.json that moon fmt then converts, leaving behind the old ones
- we need moonbit async runtime for wasi pollable
