# ArgParser

A simple command-line parser for MoonBit. It uses a declarative style to specify 
the command-line interface.

# Usage 

```moonbit
using @ArgParser {parse}
let verbose : Ref[Bool] = @ref.new(false)
let output : Ref[String] = @ref.new("output")
let files : Array[String] = []
let spec : Array[(String, String, Spec, String)] = [
  ("--verbose", "-v", Set(verbose), "enable verbose message"),
  ("--output", "-o", Set_string(output), "output file name"),
]
let usage =
  #| Simple CLI tool 
  #| usage: 
  #|      mytool [options] <file1> [<file2>] ... -o <output>
  #|

///|
test {
  let argv = ["-o", "out.mbt", "file1", "file2", "--verbose"]
  parse(spec, file => files.push(file), usage, argv)
  inspect(verbose.val, content="true")
  inspect(output.val, content="out.mbt")
  inspect(files[0], content="file1")
  inspect(files[1], content="file2")
}
```

## `help` options

ArgParser will automatically generate `--help` and `-h` options. 

```mbt
///|
test {
  let argv = ["--help"]
  parse(spec, file => files.push(file), usage, argv) catch {
    // errors raised from callbacks
    @ArgParser.ErrorMsg(msg) => println(msg)
    // errors raised from callbacks
    e => println(e)
  }
}
```

# Related Libraries

Looking for more powerful CLI tool? Check out [clap](https://mooncakes.io/docs/TheWaWaR/clap).
