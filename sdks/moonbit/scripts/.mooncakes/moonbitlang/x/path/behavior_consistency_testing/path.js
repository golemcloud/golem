import { posix as path } from "node:path";

console.log(`path.normalize('//'): '${path.normalize("//")}'`);
// path.normalize('//'): '/'

console.log(`path.dirname("a/b/"): '${path.dirname("a/b/")}'`);
// path.dirname("a/b/"): 'a'

console.log(`path.basename("a/b/"): '${path.basename("a/b/")}'`);
// path.basename("a/b/"): 'b'

console.log(`path.extname("main.mbt.md"): '${path.extname("main.mbt.md")}'`);
// path.extname("main.mbt.md"): '.md'

console.log(`path.extname("main.mbt.md/"): '${path.extname("main.mbt.md/")}'`);
// path.extname("main.mbt.md/"): '.md'

console.log(`path.relative("../..","a"): '${path.relative("../..", "a")}'`);
// this API depend on cwd absolute path

console.log(`path.join("a","/b"): '${path.join("a", "/b")}'`);
// path.join("a","/b"): 'a/b'

console.log(
  `path.win32.join("C:\\a","D:\\b"): '${path.win32.join("C:\\a", "D:\\b")}'`
);
//path.win32.join("C:\a","D:\b"): 'C:\a\D:\b'

console.log(`path.resolve("a","/b"): '${path.resolve("a","/b")}'`)
// path.resolve("a","/b"): '/b'