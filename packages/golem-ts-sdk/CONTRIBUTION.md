
### Build

```sh
 npm install
 npm run build
 npm run test
 npm run lint
 npm run format
 
```

### Things to note

* Use of `export namespace` is completely discouraged. This is mainly because RTTIST does not support it.
* File names should be either `PascalCase` or `camelCase`. `camelCase` case is preferred and `PascalCase` 
  should only be used when the file deals with a specific type that needs to have namespaced exports.
* Please be careful with imports. Follow the patterns already followed in the code.
  * Example: Use `import * as Either from 'effect/Either'` instead of `import { Either } from 'effect/Either'`.
    The latter can result in warnings about overriding `this` and unnecessary warnings.
