package main

import "fmt"
import "path"

func main() {
	fmt.Printf("path.Clean(\"//\"): '%s'\n", path.Clean("//"))
	fmt.Printf("path.Dir(\"a/b/\"): '%s'\n", path.Dir("a/b/"))
	fmt.Printf("path.Base(\"a/b/\"): '%s'\n", path.Base("a/b/"))
	fmt.Printf("path.Ext(\"main.mbt.md\"): '%s'\n", path.Ext("main.mbt.md"))
	fmt.Printf("path.Ext(\"main.mbt.md/\"): '%s'\n", path.Ext("main.mbt.md/"))
	fmt.Printf("path.Join(\"a\", \"/b\"): '%s'\n", path.Join("a", "/b"))
}
