"""Real-world usage: tldr-pages examples for the file and path commands.

Source: tldr-pages (https://github.com/tldr-pages/tldr, CC BY 4.0), pages/common and pages/linux
at commit 8cf22035e60b167ebf224c1c527b27e0b5dca24d; see ../NOTICE. Placeholders are bound to a
small project tree the script creates first. Each example also runs against the target shapes an
agent meets: a target that exists or not, a directory, a missing source, a trailing slash, a name
with a space. Output whose order is the filesystem's (find, grep -r, verbose cp/rm) is sorted.
Long listings (`ls -l`, `stat` without `-c`) and permission or owner examples are left out.
"""

TIER = "sweep"

TREE = (
    "mkdir -p /tmp/w/src/lib /tmp/w/src/empty /tmp/w/docs /tmp/w/.hidden && cd /tmp/w\n"
    "printf 'fn main() {}\\n' >src/main.rs\n"
    "printf 'pub fn add() {}\\n// TODO: sub\\n' >src/lib/math.rs\n"
    "printf '# Title\\nTODO here\\n' >docs/README.md\n"
    "printf 'secret\\n' >.hidden/key\n"
    "printf 'plain notes\\n' >notes.txt\n"
    ": >empty.txt"
)
W = "mkdir -p /tmp/w && cd /tmp/w"

CASES = []


def add(cmd, slug, body, setup=W, error=False):
    tags = ("cmd." + cmd,) + (("error",) if error else ())
    CASES.append(("real tldr " + cmd + ": " + slug, setup + "\n" + body, tags))


# -- cp -------------------------------------------------------------------------------------------
FILES = "printf 'one\\n' >a.txt && printf 'two\\n' >b.txt && mkdir dir"
add("cp", "copy a file", FILES + "\ncp a.txt c.txt && cat c.txt")
add("cp", "copy over an existing file", FILES + "\ncp a.txt b.txt && cat b.txt")
add("cp", "copy a missing file", FILES + "\ncp nope.txt c.txt", error=True)
add("cp", "copy a directory without -r", FILES + "\ncp dir d2", error=True)
add("cp", "copy into a missing directory", FILES + "\ncp a.txt nodir/c.txt", error=True)
add("cp", "copy a file onto itself", FILES + "\ncp a.txt a.txt", error=True)
add("cp", "copy a file with spaces in its name", FILES + "\nprintf 'sp\\n' >'my file.txt' && cp 'my file.txt' 'your file.txt' && cat 'your file.txt'")
add("cp", "copy an empty file", FILES + "\n: >e && cp e f && wc -c f")
add("cp", "copy into a directory", FILES + "\ncp a.txt dir && ls dir && cat dir/a.txt")
add("cp", "copy into a directory with a trailing slash", FILES + "\ncp a.txt dir/ && ls dir")
add("cp", "copy into a missing directory with a trailing slash", FILES + "\ncp a.txt nodir/", error=True)
add("cp", "copy a file onto a directory name that is a file", FILES + "\ncp a.txt b.txt/", error=True)
add("cp", "recursive copy to a new directory", TREE + "\ncp -r src copy && find copy | sort")
add("cp", "recursive copy into an existing directory", TREE + "\nmkdir dest && cp -r src dest && find dest | sort")
add("cp", "recursive copy (long)", TREE + "\ncp --recursive src copy && find copy -type f | sort")
add("cp", "recursive copy of a directory's contents", TREE + "\nmkdir dest && cp -r src/. dest && find dest | sort")
add("cp", "recursive copy with a trailing slash", TREE + "\ncp -r src/ copy && find copy -type f | sort")
add("cp", "recursive copy of hidden files", TREE + "\ncp -r .hidden h2 && cat h2/key")
add("cp", "recursive copy of an empty directory", TREE + "\ncp -r src/empty e2 && ls -a e2")
add("cp", "recursive copy of a missing directory", TREE + "\ncp -r nosrc copy", error=True)
add("cp", "recursive copy into itself", TREE + "\ncp -r src src/lib", error=True)
add("cp", "verbose recursive copy", TREE + "\ncp -vr src copy | sort")
add("cp", "verbose recursive copy (long)", TREE + "\ncp --verbose --recursive docs copy | sort")
add("cp", "verbose copy of one file", FILES + "\ncp -v a.txt c.txt")
add("cp", "copy multiple files to a target directory", FILES + "\ncp -t dir a.txt b.txt && ls dir")
add("cp", "copy multiple files to a target directory (long)", FILES + "\ncp --target-directory dir a.txt b.txt && ls dir")
add("cp", "target directory that is missing", FILES + "\ncp -t nodir a.txt", error=True)
add("cp", "copy multiple files to a non-directory", FILES + "\ncp a.txt b.txt c.txt", error=True)
add("cp", "interactive copy declined", FILES + "\ncp a.txt dir/ && printf 'new\\n' >a.txt && echo n | cp -i *.txt dir; cat dir/a.txt")
add("cp", "interactive copy accepted", FILES + "\ncp a.txt dir/ && printf 'new\\n' >a.txt && echo y | cp -i a.txt dir; cat dir/a.txt")
add("cp", "interactive copy (long)", FILES + "\necho n | cp --interactive a.txt b.txt; cat b.txt")
add("cp", "interactive copy with no conflict", FILES + "\ncp -i a.txt dir && cat dir/a.txt")
add("cp", "follow a symbolic link", FILES + "\nln -s a.txt link && cp -L link dir && cat dir/link && [ -L dir/link ] || echo not-a-link")
add("cp", "follow a symbolic link (long)", FILES + "\nln -s a.txt link && cp --dereference link dir && cat dir/link")
add("cp", "copy a symbolic link without -L", FILES + "\nln -s a.txt link && cp link c && cat c")
add("cp", "copy with parents", TREE + "\nmkdir dest && cp --parents src/lib/math.rs dest && find dest | sort")
add("cp", "copy with parents to a missing directory", TREE + "\ncp --parents src/lib/math.rs nodest", error=True)

# -- mv -------------------------------------------------------------------------------------------
add("mv", "rename a file", FILES + "\nmv a.txt c.txt && ls && cat c.txt")
add("mv", "rename a directory", FILES + "\nmv dir dir2 && ls")
add("mv", "rename over an existing file", FILES + "\nmv a.txt b.txt && ls && cat b.txt")
add("mv", "rename a missing file", FILES + "\nmv nope.txt c.txt", error=True)
add("mv", "rename into a missing directory", FILES + "\nmv a.txt nodir/c.txt", error=True)
add("mv", "rename a file onto itself", FILES + "\nmv a.txt a.txt", error=True)
add("mv", "rename a file with spaces", FILES + "\nmv a.txt 'new name.txt' && ls")
add("mv", "move into a directory", FILES + "\nmv a.txt dir && ls . dir")
add("mv", "move into a directory with a trailing slash", FILES + "\nmv a.txt dir/ && ls dir")
add("mv", "move a directory into itself", FILES + "\nmv dir dir/sub", error=True)
add("mv", "move a directory over a file", FILES + "\nmv dir a.txt", error=True)
add("mv", "move a file over a directory", FILES + "\nmkdir -p d2/a.txt && mv a.txt d2", error=True)
add("mv", "move multiple files into a directory", FILES + "\nmv a.txt b.txt dir && ls dir")
add("mv", "move multiple files to a non-directory", FILES + "\nmv a.txt b.txt c.txt", error=True)
add("mv", "force", FILES + "\nmv -f a.txt b.txt && cat b.txt")
add("mv", "force (long)", FILES + "\nmv --force a.txt b.txt && cat b.txt")
add("mv", "interactive declined", FILES + "\necho n | mv -i a.txt b.txt; cat a.txt b.txt")
add("mv", "interactive accepted", FILES + "\necho y | mv -i a.txt b.txt; ls; cat b.txt")
add("mv", "interactive (long)", FILES + "\necho n | mv --interactive a.txt b.txt; ls")
add("mv", "no clobber", FILES + "\nmv -n a.txt b.txt; echo \"status=$?\"; cat a.txt b.txt")
add("mv", "no clobber (long)", FILES + "\nmv --no-clobber a.txt b.txt; echo \"status=$?\"; ls")
add("mv", "no clobber without a conflict", FILES + "\nmv -n a.txt c.txt; echo \"status=$?\"; ls")
add("mv", "verbose", FILES + "\nmv -v a.txt c.txt")
add("mv", "verbose (long)", FILES + "\nmv --verbose a.txt b.txt dir")
add("mv", "target directory from find and xargs", TREE + "\nmkdir logs && touch x.log y.log src/z.log && find . -path ./logs -prune -o -type f -name '*.log' -print0 | xargs -0 mv -t logs && ls logs")
add("mv", "target directory (long)", FILES + "\nmv --target-directory dir a.txt b.txt && ls dir")
add("mv", "target directory with no input", FILES + "\nprintf '' | xargs -0 mv -t dir; echo \"status=$?\"")

# -- rm -------------------------------------------------------------------------------------------
add("rm", "remove files", FILES + "\nrm a.txt b.txt && ls")
add("rm", "remove a missing file", FILES + "\nrm nope.txt; echo \"status=$?\"; ls", error=True)
add("rm", "remove a directory without -r", FILES + "\nrm dir", error=True)
add("rm", "remove files and a missing one", FILES + "\nrm a.txt nope.txt b.txt; echo \"status=$?\"; ls", error=True)
add("rm", "force ignores missing files", FILES + "\nrm -f a.txt nope.txt && ls")
add("rm", "force (long)", FILES + "\nrm --force nope.txt && echo ok")
add("rm", "force with no operands", FILES + "\nrm -f && echo ok")
add("rm", "no operands", FILES + "\nrm", error=True)
add("rm", "interactive declined", FILES + "\necho n | rm -i a.txt; ls")
add("rm", "interactive accepted", FILES + "\necho y | rm -i a.txt; ls")
add("rm", "interactive (long)", FILES + "\nprintf 'y\\nn\\n' | rm --interactive a.txt b.txt; ls")
add("rm", "verbose", FILES + "\nrm -v a.txt b.txt")
add("rm", "verbose (long)", FILES + "\nrm --verbose a.txt")
add("rm", "recursive", TREE + "\nrm -r src notes.txt && ls -A")
add("rm", "recursive (long)", TREE + "\nrm --recursive docs && ls")
add("rm", "recursive verbose", TREE + "\nrm -rv src | sort")
add("rm", "recursive on a missing path", TREE + "\nrm -r nosuch", error=True)
add("rm", "recursive force on a missing path", TREE + "\nrm -rf nosuch && echo ok")
add("rm", "remove the current directory", TREE + "\nrm -r .", error=True)
add("rm", "empty directory", TREE + "\nrm -d src/empty && ls src")
add("rm", "empty directory (long)", TREE + "\nrm --dir src/empty && ls src")
add("rm", "non-empty directory with -d", TREE + "\nrm -d src", error=True)
add("rm", "a file with -d", TREE + "\nrm -d notes.txt && ls")
add("rm", "a file named with a dash", FILES + "\n: >-x && rm -- -x && ls")

# -- rmdir ----------------------------------------------------------------------------------------
add("rmdir", "remove directories", TREE + "\nmkdir d1 d2 && rmdir d1 d2 && ls")
add("rmdir", "remove a non-empty directory", TREE + "\nrmdir src", error=True)
add("rmdir", "remove a missing directory", TREE + "\nrmdir nosuch", error=True)
add("rmdir", "remove a file", TREE + "\nrmdir notes.txt", error=True)
add("rmdir", "remove with a trailing slash", TREE + "\nrmdir src/empty/ && ls src")
add("rmdir", "remove nested directories with parents", W + "\nmkdir -p a/b/c && rmdir -p a/b/c && ls -A")
add("rmdir", "remove nested directories with parents (long)", W + "\nmkdir -p a/b/c && rmdir --parents a/b/c && ls -A")
add("rmdir", "parents stops at a non-empty directory", W + "\nmkdir -p a/b/c && : >a/f && rmdir -p a/b/c; echo \"status=$?\"; find a | sort", error=True)
add("rmdir", "clean a directory of empty directories", W + "\nmkdir e1 e2 full && : >full/f && : >file && rmdir *; echo \"status=$?\"; ls", error=True)

# -- mkdir ----------------------------------------------------------------------------------------
add("mkdir", "create directories", W + "\nmkdir d1 d2 && ls")
add("mkdir", "create an existing directory", W + "\nmkdir d1 && mkdir d1", error=True)
add("mkdir", "create under a missing parent", W + "\nmkdir a/b", error=True)
add("mkdir", "create over a file", W + "\n: >f && mkdir f", error=True)
add("mkdir", "create with parents", W + "\nmkdir -p a/b c/d && find . | sort")
add("mkdir", "create with parents (long)", W + "\nmkdir --parents a/b && find a | sort")
add("mkdir", "parents of an existing directory", W + "\nmkdir -p a && mkdir -p a && echo ok")
add("mkdir", "parents through a file", W + "\n: >f && mkdir -p f/g", error=True)
add("mkdir", "nested brace expansion", W + "\nmkdir -p path/{a,b}/{x,y,z}/{h,i,j} && find path -type d | wc -l && ls path/b/z")
add("mkdir", "verbose parents", W + "\nmkdir -pv a/b/c")
add("mkdir", "name with spaces", W + "\nmkdir 'my dir' && ls")

# -- ln / link / unlink ---------------------------------------------------------------------------
add("ln", "relative symbolic link", FILES + "\nln -s a.txt link && readlink link && cat link")
add("ln", "relative symbolic link (long)", FILES + "\nln --symbolic a.txt link && cat link")
add("ln", "symbolic link to a directory", FILES + "\nln -s dir dlink && touch dir/x && ls dlink")
add("ln", "symbolic link in a subdirectory", FILES + "\nln -s ../a.txt dir/up && cat dir/up")
add("ln", "dangling symbolic link", FILES + "\nln -s nope.txt link && readlink link; cat link", error=True)
add("ln", "symbolic link over an existing file", FILES + "\nln -s a.txt b.txt", error=True)
add("ln", "force a symbolic link", FILES + "\nln -s a.txt link && ln -sf b.txt link && cat link")
add("ln", "force a symbolic link (long)", FILES + "\nln -s a.txt link && ln --symbolic --force b.txt link && readlink link")
add("ln", "hard link", FILES + "\nln /tmp/w/a.txt hard && cat hard && [ a.txt -ef hard ] && echo same")
add("ln", "hard link to a missing file", FILES + "\nln /tmp/w/nope.txt hard", error=True)
add("ln", "hard link to a directory", FILES + "\nln dir hard", error=True)
add("ln", "hard link into a directory", FILES + "\nln a.txt dir && cat dir/a.txt")
add("link", "hard link", FILES + "\nlink a.txt hard && cat hard")
add("link", "existing new name", FILES + "\nlink a.txt b.txt", error=True)
add("link", "missing existing file", FILES + "\nlink nope.txt hard", error=True)
add("link", "extra operand", FILES + "\nlink a.txt b c", error=True)
add("unlink", "remove a file", FILES + "\nunlink a.txt && ls")
add("unlink", "remove a missing file", FILES + "\nunlink nope.txt", error=True)
add("unlink", "remove a directory", FILES + "\nunlink dir", error=True)
add("unlink", "remove a symbolic link", FILES + "\nln -s a.txt link && unlink link && ls")

# -- touch ----------------------------------------------------------------------------------------
TS = "date -r {} +%Y-%m-%dT%H:%M:%S"
add("touch", "create files", W + "\ntouch a b && ls")
add("touch", "touch an existing file keeps its content", W + "\necho keep >a && touch a && cat a")
add("touch", "no create with access time", W + "\ntouch -c -a a b; echo \"status=$?\"; ls")
add("touch", "no create with modification time", W + "\ntouch a && touch -c -m a b; echo \"status=$?\"; ls")
add("touch", "no create (long)", W + "\ntouch --no-create -m a; echo \"status=$?\"; ls")
add("touch", "specific time", W + "\ntouch a && touch -c -t 202001011200.30 a b && ls && " + TS.format("a"))
add("touch", "specific time without seconds", W + "\ntouch -t 202001011200 a && " + TS.format("a"))
add("touch", "specific time with a century-less year", W + "\ntouch -t 9912312359 a && " + TS.format("a"))
add("touch", "invalid specific time", W + "\ntouch -t 2020 a", error=True)
add("touch", "reference file", W + "\ntouch -t 201506070809 ref && touch a && touch -c -r ref a b && ls && " + TS.format("a"))
add("touch", "reference file (long)", W + "\ntouch -t 201506070809 ref && touch --no-create --reference ref a; ls; touch --reference ref a && " + TS.format("a"))
add("touch", "missing reference file", W + "\ntouch -r nope a", error=True)
for _d in ("2020-11-14", "14 nov 2020", "2020-11-14 10:30:00", "@1600000000", "2020-11-14T10:30:00Z", "nov 14 2020 10:30"):
    add("touch", "parse a date string " + _d, W + "\ntouch -d '" + _d + "' a && " + TS.format("a"))
add("touch", "parse a date string (long)", W + "\ntouch --date '2021-03-04 05:06' a && " + TS.format("a"))
add("touch", "invalid date string", W + "\ntouch -d 'not a date' a", error=True)
add("touch", "increasing numbers", W + "\ntouch file{1..10} && ls")
add("touch", "letter range", W + "\ntouch file{a..e} && ls")
add("touch", "into a missing directory", W + "\ntouch nodir/a", error=True)
add("touch", "order by time", W + "\ntouch -d 2020-01-03 c && touch -d 2020-01-01 a && touch -d 2020-01-02 b && ls -t && ls -tr")

# -- truncate -------------------------------------------------------------------------------------
SZ = "stat -c '%s %n' f"
for _s in ("10K", "10KB", "1M", "1G", "3", "0"):
    add("truncate", "set a size of " + _s, W + "\nprintf 'hello world\\n' >f && truncate -s " + _s + " f && " + SZ)
add("truncate", "create a new file with a size", W + "\ntruncate -s 10K f && " + SZ)
add("truncate", "set a size (long)", W + "\ntruncate --size 2K f && " + SZ)
add("truncate", "extend a file", W + "\nprintf 'hello\\n' >f && truncate -s +50K f && " + SZ + " && head -c 8 f | od -c")
add("truncate", "shrink a file", W + "\ntruncate -s 5000 f && truncate -s -2K f && " + SZ)
add("truncate", "shrink below zero", W + "\nprintf 'abc' >f && truncate -s -2K f && " + SZ)
add("truncate", "empty a file", W + "\nprintf 'hello\\n' >f && truncate -s 0 f && " + SZ)
add("truncate", "empty without creating", W + "\ntruncate -s 0 -c f; echo \"status=$?\"; ls")
add("truncate", "empty without creating (long)", W + "\nprintf 'x' >f && truncate --size 0 --no-create f g && ls && " + SZ)
add("truncate", "round up to a multiple", W + "\nprintf 'hello' >f && truncate -s %4 f && " + SZ)
add("truncate", "round down to a multiple", W + "\nprintf 'hello world' >f && truncate -s /4 f && " + SZ)
add("truncate", "invalid size", W + "\ntruncate -s 10Q f", error=True)
add("truncate", "a directory", W + "\nmkdir d && truncate -s 0 d", error=True)

# -- ls -------------------------------------------------------------------------------------------
add("ls", "one per line", TREE + "\nls -1")
add("ls", "one per line of a subdirectory", TREE + "\nls -1 src")
add("ls", "all files", TREE + "\nls -a")
add("ls", "all files (long)", TREE + "\nls --all")
add("ls", "almost all files", TREE + "\nls -A")
add("ls", "classify", TREE + "\nln -s notes.txt link && ls -F")
add("ls", "classify (long)", TREE + "\nls --classify src")
add("ls", "sorted by size recursively", W + "\nmkdir -p s/t && printf '1' >s/small && printf '12345' >s/big && printf '123' >s/t/mid && printf '12' >s/t/two && cd s && ls -S big small && ls -S t && ls -R")
add("ls", "sorted by size in reverse", W + "\nprintf '1' >a && printf '12345' >b && printf '123' >c && ls -Sr")
add("ls", "sorted by time reversed", W + "\ntouch -d 2020-01-03 c && touch -d 2020-01-01 a && touch -d 2020-01-02 b && ls -tr")
add("ls", "sorted by time reversed (long)", W + "\ntouch -d 2020-01-03 c && touch -d 2020-01-01 a && touch -d 2020-01-02 b && ls -t --reverse")
add("ls", "recursive", TREE + "\nls -R")
add("ls", "recursive (long)", TREE + "\nls --recursive src")
add("ls", "only directories", TREE + "\nls -d */")
add("ls", "only directories (long)", TREE + "\nls --directory */")
add("ls", "only directories when there are none", W + "\ntouch f && ls -d */", error=True)
add("ls", "a missing path", TREE + "\nls nosuch", error=True)
add("ls", "a missing path among others", TREE + "\nls nosuch notes.txt src", error=True)
add("ls", "one file", TREE + "\nls notes.txt")
add("ls", "names needing quotes", W + "\ntouch 'a b' \"it's\" 'x*y' && ls")
add("ls", "one per line of names needing quotes", W + "\ntouch 'a b' \"it's\" && ls -1")
add("ls", "reverse", TREE + "\nls -r")
add("ls", "comma separated", TREE + "\nls -m")
add("ls", "directory itself", TREE + "\nls -d src")
add("ls", "several directories", TREE + "\nls src docs")
add("ls", "hidden directory", TREE + "\nls -a .hidden")
add("ls", "sort by extension", W + "\ntouch b.txt a.rs c.md d && ls -X")
add("ls", "natural version sort", W + "\ntouch f10 f9 f1 f100 && ls -v")

# -- find -----------------------------------------------------------------------------------------
add("find", "files by extension", TREE + "\nfind . -name '*.rs' | sort")
add("find", "files by extension from a path", TREE + "\nfind src -name '*.rs' | sort")
add("find", "files by extension with an absolute path", TREE + "\nfind /tmp/w/src -name '*.rs' | sort")
add("find", "files by extension with no match", TREE + "\nfind . -name '*.py'; echo \"status=$?\"")
add("find", "files by extension in a missing directory", TREE + "\nfind nosuch -name '*.rs'", error=True)
add("find", "unquoted pattern that matches in the cwd", TREE + "\ntouch x.rs && find . -name *.rs | sort")
add("find", "multiple path and name patterns", TREE + "\nfind . -path '*/lib/*.rs' -or -name '*note*' | sort")
add("find", "multiple path and name patterns with -o", TREE + "\nfind . -path '*/src/*' -o -name '*.md' | sort")
add("find", "directories case-insensitively", TREE + "\nfind . -type d -iname '*LIB*'")
add("find", "directories case-insensitively with no match", TREE + "\nfind . -type d -iname '*NONE*' | wc -l")
add("find", "excluding a path", TREE + "\nfind . -name '*.rs' -not -path '*/lib/*'")
add("find", "excluding a path with !", TREE + "\nfind . -type f ! -path './src/*' | sort")
add("find", "size range at depth 1", W + "\ntruncate -s 600K big.bin && truncate -s 20M huge.bin && truncate -s 100K small.bin && mkdir d && truncate -s 700K d/deep.bin && find . -maxdepth 1 -size +500k -size -10M")
add("find", "size in blocks and bytes", W + "\nprintf '1234' >f4 && : >f0 && truncate -s 1000 f1000 && find . -type f -size -2 | sort && find . -type f -size 4c && find . -type f -size +999c")
add("find", "exec per file", TREE + "\nfind . -name '*.rs' -exec wc -l {} \\; | sort")
add("find", "exec per file on a missing command", TREE + "\nfind . -name 'main.rs' -exec nosuchcmd {} \\;; echo \"status=$?\"")
add("find", "exec per file that fails", TREE + "\nfind . -name '*.rs' -exec grep -q nothing {} \\; ; echo \"status=$?\"")
add("find", "files modified today to one command", TREE + "\nfind . -daystart -mtime -1 -type f -exec ls {} + ")
add("find", "files modified long ago", TREE + "\ntouch -d 2001-01-01 notes.txt && find . -type f -mtime +365")
add("find", "empty files deleted verbosely", TREE + "\nfind . -type f -empty -delete -print && ls")
add("find", "empty directories deleted verbosely", TREE + "\nmkdir -p x/y && find . -type d -empty -delete -print | sort && ls")
add("find", "empty entries", TREE + "\nfind . -empty | sort")
add("find", "maxdepth and mindepth", TREE + "\nfind . -mindepth 2 -maxdepth 2 | sort")
add("find", "type l", TREE + "\nln -s notes.txt link && find . -type l")
add("find", "follow links", TREE + "\nln -s src slink && find -L . -name 'main.rs' | sort")
add("find", "name with a character class", TREE + "\nfind . -name '[mn]*' | sort")
add("find", "regex", TREE + "\nfind . -regex '.*/[a-z]+\\.rs' | sort")
add("find", "print0 to xargs", TREE + "\nfind . -name '*.rs' -print0 | xargs -0 grep -c fn | sort")
add("find", "prune a directory", TREE + "\nfind . -path ./src -prune -o -type f -print | sort")
add("find", "newer than a file", TREE + "\ntouch -d 2001-01-01 old && touch -d 2002-01-01 notes.txt && find . -newer old -name '*.txt' | sort")

# -- stat / file / realpath / readlink / basename / dirname ---------------------------------------
add("stat", "size and name", W + "\nprintf 'hello' >f && stat -c \"%s %n\" f")
add("stat", "size and name (long)", W + "\nprintf 'hello' >f && stat --format \"%s %n\" f")
add("stat", "size and name of several files", W + "\nprintf 'hello' >f && : >e && stat -c \"%s %n\" f e")
add("stat", "size of a missing file", W + "\nstat -c \"%s %n\" nope", error=True)
add("stat", "file type names", W + "\nprintf 'x' >f && : >e && mkdir d && ln -s f l && stat -c '%F %n' f e d l")
add("stat", "printf format with escapes", W + "\nprintf 'hello' >f && stat --printf '%s\\t%n\\n' f")
add("stat", "link target", W + "\nprintf 'x' >f && ln -s f l && stat -c '%N' l")

FT = [
    ("ascii text", "printf 'hello world\\n' >t"),
    ("empty", ": >t"),
    ("utf-8 text", "printf 'caf\\303\\251 na\\303\\257ve\\n' >t"),
    ("crlf text", "printf 'a\\r\\nb\\r\\n' >t"),
    ("no final newline", "printf 'abc' >t"),
    ("shell script", "printf '#!/bin/sh\\necho hi\\n' >t"),
    ("json", "printf '{\"a\": [1, 2]}\\n' >t"),
    ("gzip", "printf '\\037\\213\\010\\000\\000\\000\\000\\000\\000\\003' >t"),
    ("png", "printf '\\211PNG\\r\\n\\032\\n\\000\\000\\000\\rIHDR\\000\\000\\000\\001\\000\\000\\000\\001\\010\\006\\000\\000\\000' >t"),
    ("pdf", "printf '%%PDF-1.4\\n' >t"),
    ("zip", "printf 'PK\\003\\004\\024\\000\\000\\000' >t"),
    ("binary zeros", "truncate -s 64 t"),
    ("directory", "mkdir t"),
    ("symbolic link", "printf 'x\\n' >target && ln -s target t"),
]
for _label, _setup in FT:
    add("file", "describe " + _label, W + "\n" + _setup + "\nfile t")
    add("file", "brief " + _label, W + "\n" + _setup + "\nfile -b t")
    add("file", "mime " + _label, W + "\n" + _setup + "\nfile -i t")
add("file", "brief (long)", W + "\nprintf 'hello\\n' >t && file --brief t")
add("file", "mime (long)", W + "\nprintf 'hello\\n' >t && file --mime t")
add("file", "uncompress a zip", W + "\nprintf 'PK\\003\\004\\024\\000\\000\\000' >t.zip && file -z t.zip")
add("file", "uncompress (long)", W + "\nprintf 'hello\\n' >t && file --uncompress t")
add("file", "special files", W + "\nprintf 'hello\\n' >t && file -s t")
add("file", "special files (long)", W + "\nprintf 'hello\\n' >t && file --special-files /dev/null")
add("file", "keep going", W + "\nprintf 'hello\\n' >t && file -k t")
add("file", "keep going (long)", W + "\nprintf '{\"a\":1}\\n' >t && file --keep-going t")
add("file", "several files", W + "\nprintf 'hello\\n' >a && : >b && mkdir c && file a b c")
add("file", "a missing file", W + "\nfile nope; echo \"status=$?\"")
add("file", "names from a pipe with -f", W + "\nprintf 'hello\\n' >a && : >b && printf 'a\\nb\\n' | file -f -")

LINKS = "mkdir -p real/sub && printf 'x\\n' >real/f && ln -s real rl && ln -s real/f fl && ln -s nope dangling"
add("readlink", "target of a link", W + "\n" + LINKS + "\nreadlink fl && readlink rl")
add("readlink", "target of a dangling link", W + "\n" + LINKS + "\nreadlink dangling")
add("readlink", "target of a regular file", W + "\n" + LINKS + "\nreadlink real/f", error=True)
add("readlink", "target of a missing path", W + "\n" + LINKS + "\nreadlink nope", error=True)
for _p in ("rl", "fl", "rl/sub/..", "real/./sub/../f", "dangling", "nope/x", "rl/nope", "."):
    add("readlink", "canonicalize " + _p, W + "\n" + LINKS + "\nreadlink -f " + _p + "; echo \"status=$?\"")
add("readlink", "canonicalize (long)", W + "\n" + LINKS + "\nreadlink --canonicalize rl/sub")
add("readlink", "canonicalize existing", W + "\n" + LINKS + "\nreadlink -e dangling; echo \"status=$?\"; readlink -e fl")
add("readlink", "canonicalize missing", W + "\n" + LINKS + "\nreadlink -m nope/x/../y")
add("readlink", "no newline", W + "\n" + LINKS + "\nreadlink -n fl; echo '|'")
for _p in ("real/f", "rl", "fl", "rl/sub/..", "real/./sub/../f", "dangling", "nope", "nope/x", ".", "/tmp/w//real/"):
    add("realpath", "absolute path of " + _p, W + "\n" + LINKS + "\nrealpath " + _p + "; echo \"status=$?\"")
add("realpath", "require all components", W + "\n" + LINKS + "\nrealpath -e real/f && realpath -e nope/x; echo \"status=$?\"")
add("realpath", "require all components (long)", W + "\n" + LINKS + "\nrealpath --canonicalize-existing dangling; echo \"status=$?\"")
add("realpath", "logical", W + "\n" + LINKS + "\nrealpath -L rl/sub/..")
add("realpath", "logical (long)", W + "\n" + LINKS + "\nrealpath --logical rl/..")
add("realpath", "no symlinks", W + "\n" + LINKS + "\nrealpath -s rl/sub/.. fl")
add("realpath", "no symlinks (long)", W + "\n" + LINKS + "\nrealpath --no-symlinks rl")
add("realpath", "quiet", W + "\n" + LINKS + "\nrealpath -q -e nope; echo \"status=$?\"")
add("realpath", "quiet (long)", W + "\n" + LINKS + "\nrealpath --quiet -e nope; echo \"status=$?\"")
add("realpath", "relative to", W + "\n" + LINKS + "\nrealpath --relative-to=real/sub real/f")
add("realpath", "several paths", W + "\n" + LINKS + "\nrealpath real fl nope")

for _p in ("path/to/file.txt", "path/to/dir/", "file", "/", "//", "", "a//b//", "/usr/lib/", ".hidden", "-"):
    add("basename", "file name of '" + _p + "'", W + "\nbasename -- '" + _p + "'; echo \"status=$?\"")
for _p, _s in (("path/to/file.txt", ".txt"), ("file.txt", "file.txt"), ("archive.tar.gz", ".gz"), ("x.txt", ".md"), ("dir/", "r")):
    add("basename", "remove suffix " + _s + " from " + _p, W + "\nbasename '" + _p + "' '" + _s + "'")
add("basename", "several with -a", W + "\nbasename -a a/b c/d/ e")
add("basename", "suffix with -s", W + "\nbasename -s .rs src/main.rs src/lib.rs")
add("basename", "zero terminated", W + "\nbasename -z a/b | od -c")
add("basename", "no operand", W + "\nbasename", error=True)
add("basename", "extra operand", W + "\nbasename a b c", error=True)
for _p in ("path/to/file", "path/to/dir/", "file", "/", "//", "/usr", "a//b//", "", "./x", "../x/y"):
    add("dirname", "parent of '" + _p + "'", W + "\ndirname -- '" + _p + "'")
add("dirname", "several paths", W + "\ndirname a/b c/d/e /f")
add("dirname", "zero delimited", W + "\ndirname -z a/b c/d | od -c")
add("dirname", "zero delimited (long)", W + "\ndirname --zero a/b | tr '\\0' '\\n'")
add("dirname", "no operand", W + "\ndirname", error=True)

# -- mktemp ---------------------------------------------------------------------------------------
SHAPE = 'case $f in {}) echo "name ok";; *) echo "unexpected $f";; esac; [ {} "$f" ] && echo exists'
add("mktemp", "empty temporary file", W + "\nf=$(mktemp) && " + SHAPE.format("/tmp/tmp.??????????", "-f"))
add("mktemp", "custom directory", W + "\nmkdir tdir && f=$(mktemp -p /tmp/w/tdir) && " + SHAPE.format("/tmp/w/tdir/tmp.??????????", "-f"))
add("mktemp", "custom directory (long)", W + "\nmkdir tdir && f=$(mktemp --tmpdir=/tmp/w/tdir) && " + SHAPE.format("/tmp/w/tdir/tmp.??????????", "-f"))
add("mktemp", "custom directory that is missing", W + "\nmktemp -p /tmp/w/nodir >/dev/null; echo \"status=$?\"")
add("mktemp", "custom path template", W + "\nf=$(mktemp /tmp/example.XXXXXXXX) && " + SHAPE.format("/tmp/example.????????", "-f"))
add("mktemp", "custom file name template", W + "\nf=$(mktemp -t example.XXXXXXXX) && " + SHAPE.format("/tmp/example.????????", "-f"))
add("mktemp", "template with too few Xs", W + "\nmktemp /tmp/example.XX >/dev/null; echo \"status=$?\"")
add("mktemp", "template with a relative path", W + "\nf=$(mktemp rel.XXXXXX) && " + SHAPE.format("rel.??????", "-f"))
add("mktemp", "suffix", W + "\nf=$(mktemp --suffix .ext) && " + SHAPE.format("/tmp/tmp.??????????.ext", "-f"))
add("mktemp", "directory", W + "\nf=$(mktemp -d) && " + SHAPE.format("/tmp/tmp.??????????", "-d"))
add("mktemp", "directory (long)", W + "\nf=$(mktemp --directory) && " + SHAPE.format("/tmp/tmp.??????????", "-d"))
add("mktemp", "dry run", W + "\nf=$(mktemp -u) && " + SHAPE.format("/tmp/tmp.??????????", "-e") + " || echo absent")
add("mktemp", "dry run (long)", W + "\nf=$(mktemp --dry-run) && " + SHAPE.format("/tmp/tmp.??????????", "-e") + " || echo absent")
add("mktemp", "two calls give two names", W + "\na=$(mktemp) && b=$(mktemp) && [ \"$a\" != \"$b\" ] && echo distinct")
add("mktemp", "quiet failure", W + "\nmktemp -q /nodir/x.XXXXXX; echo \"status=$?\"")

# -- diff / cmp / patch ---------------------------------------------------------------------------
OLDNEW = "printf 'one\\ntwo\\nthree\\nfour\\n' >old && printf 'one\\n2\\nthree  \\nfour\\nfive\\n' >new && touch -d '2020-01-01 00:00:00' old new"
DIFFSHAPES = [
    ("", OLDNEW),
    (" of identical files", "printf 'a\\nb\\n' >old && cp old new && touch -d '2020-01-01 00:00:00' old new"),
    (" with a missing file", "printf 'a\\n' >old && touch -d '2020-01-01 00:00:00' old"),
    (" without final newlines", "printf 'a\\nb' >old && printf 'a\\nc' >new && touch -d '2020-01-01 00:00:00' old new"),
    (" of an empty file", ": >old && printf 'a\\n' >new && touch -d '2020-01-01 00:00:00' old new"),
    (" with CRLF", "printf 'a\\r\\nb\\r\\n' >old && printf 'a\\nb\\n' >new && touch -d '2020-01-01 00:00:00' old new"),
    (" of binary files", "printf 'a\\000b' >old && printf 'a\\000c' >new && touch -d '2020-01-01 00:00:00' old new"),
    (" of whitespace-only changes", "printf 'a b\\n  c\\n' >old && printf 'a  b\\nc\\n' >new && touch -d '2020-01-01 00:00:00' old new"),
]
for _suffix, _setup in DIFFSHAPES:
    add("diff", "normal" + _suffix, W + "\n" + _setup + "\ndiff old new")
    add("diff", "ignoring white space" + _suffix, W + "\n" + _setup + "\ndiff -w old new")
    add("diff", "side by side" + _suffix, W + "\n" + _setup + "\ndiff -y old new")
    add("diff", "unified" + _suffix, W + "\n" + _setup + "\ndiff -u old new")
    add("diff", "patch treating missing files as empty" + _suffix, W + "\n" + _setup + "\ndiff -a -u -N old new > diff.patch; echo \"status=$?\"; cat diff.patch")
    add("diff", "minimal in color" + _suffix, W + "\n" + _setup + "\ndiff -d --color=always old new")
add("diff", "ignoring white space (long)", W + "\n" + OLDNEW + "\ndiff --ignore-all-space old new")
add("diff", "side by side (long)", W + "\n" + OLDNEW + "\ndiff --side-by-side old new")
add("diff", "unified (long)", W + "\n" + OLDNEW + "\ndiff --unified old new")
add("diff", "patch (long)", W + "\n" + OLDNEW + "\ndiff --text --unified --new-file old nope")
add("diff", "minimal (long)", W + "\n" + OLDNEW + "\ndiff --minimal --color=always old new")
DIRS = (
    "mkdir -p a/sub b/sub a/only && printf '1\\n' >a/f && printf '2\\n' >b/f && printf 's\\n' >a/sub/s && "
    "printf 's\\n' >b/sub/s && printf 'x\\n' >b/newfile && touch -d '2020-01-01 00:00:00' a/f b/f b/newfile"
)
add("diff", "directories recursively", W + "\n" + DIRS + "\ndiff -r a b")
add("diff", "directories recursively (long)", W + "\n" + DIRS + "\ndiff --recursive a b")
add("diff", "directories brief", W + "\n" + DIRS + "\ndiff -r -q a b")
add("diff", "directories brief (long)", W + "\n" + DIRS + "\ndiff --recursive --brief a b")
add("diff", "directories unified new-file", W + "\n" + DIRS + "\ndiff -ruN a b")
add("diff", "identical directories", W + "\nmkdir a b && printf '1\\n' >a/f && cp a/f b/f && diff -r a b && echo same")
add("diff", "directory and file", W + "\n" + DIRS + "\ndiff a/f b && echo ok")
add("diff", "directories without -r", W + "\n" + DIRS + "\ndiff a b")

CMPSHAPES = [
    ("", "printf 'abcdef\\nline2\\n' >f1 && printf 'abXdef\\nline2\\n' >f2"),
    (" of identical files", "printf 'same\\n' >f1 && cp f1 f2"),
    (" where one is a prefix", "printf 'abc' >f1 && printf 'abcdef' >f2"),
    (" of an empty file", ": >f1 && printf 'x' >f2"),
    (" on a later line", "printf 'a\\nb\\nc\\n' >f1 && printf 'a\\nb\\nd\\n' >f2"),
    (" with a missing file", "printf 'a' >f1"),
    (" of high bytes", "printf 'a\\351b' >f1 && printf 'a\\352b' >f2"),
]
for _suffix, _setup in CMPSHAPES:
    add("cmp", "first difference" + _suffix, W + "\n" + _setup + "\ncmp f1 f2")
    add("cmp", "print bytes" + _suffix, W + "\n" + _setup + "\ncmp -b f1 f2")
    add("cmp", "every difference" + _suffix, W + "\n" + _setup + "\ncmp -l f1 f2")
    add("cmp", "quiet" + _suffix, W + "\n" + _setup + "\ncmp -s f1 f2; echo \"status=$?\"")
add("cmp", "print bytes (long)", W + "\n" + CMPSHAPES[0][1] + "\ncmp --print-bytes f1 f2")
add("cmp", "every difference (long)", W + "\n" + CMPSHAPES[0][1] + "\ncmp --verbose f1 f2")
add("cmp", "quiet (long)", W + "\n" + CMPSHAPES[0][1] + "\ncmp --quiet f1 f2; echo \"status=$?\"")

PATCHSET = (
    "mkdir -p proj/src && printf 'a\\nb\\nc\\nd\\ne\\n' >proj/src/f && cp proj/src/f proj/src/f.orig && "
    "printf 'a\\nB\\nc\\nd\\nE\\n' >proj/src/f.new && "
    "(cd proj && diff -u --label a/src/f --label b/src/f src/f.orig src/f.new >../p1.diff); "
    "(cd proj/src && diff -u --label f --label f f.orig f.new >../../p0.diff); rm proj/src/f.orig proj/src/f.new"
)
add("patch", "apply with names from the diff", W + "\n" + PATCHSET + "\ncd proj/src && patch -i ../../p0.diff && cat f")
add("patch", "apply with names from the diff (long)", W + "\n" + PATCHSET + "\ncd proj/src && patch --input ../../p0.diff && cat f")
add("patch", "apply to a specific file", W + "\n" + PATCHSET + "\ncp proj/src/f target && patch -i p0.diff target && cat target")
add("patch", "apply to a specific file (long)", W + "\n" + PATCHSET + "\ncp proj/src/f target && patch --input p0.diff target && cat target")
add("patch", "write the result to another file", W + "\n" + PATCHSET + "\npatch -i p0.diff proj/src/f -o out && cat out && cat proj/src/f")
add("patch", "write the result to another file (long)", W + "\n" + PATCHSET + "\npatch --input p0.diff proj/src/f --output out && cat out")
add("patch", "strip one component", W + "\n" + PATCHSET + "\ncd proj && patch -i ../p1.diff -p 1 && cat src/f")
add("patch", "strip one component (long)", W + "\n" + PATCHSET + "\ncd proj && patch --input ../p1.diff --strip 1 && cat src/f")
add("patch", "strip zero components on an a/ b/ diff", W + "\n" + PATCHSET + "\ncd proj && patch -p0 -i ../p1.diff; echo \"status=$?\"", error=True)
add("patch", "reverse", W + "\n" + PATCHSET + "\ncd proj/src && patch -i ../../p0.diff && patch -i ../../p0.diff -R && cat f")
add("patch", "reverse (long)", W + "\n" + PATCHSET + "\ncd proj/src && patch -i ../../p0.diff && patch --input ../../p0.diff --reverse && cat f")
add("patch", "already applied", W + "\n" + PATCHSET + "\ncd proj/src && patch -i ../../p0.diff && patch -i ../../p0.diff; echo \"status=$?\"; ls", error=True)
add("patch", "apply with an offset", W + "\n" + PATCHSET + "\ncd proj/src && printf 'x\\ny\\n' | cat - f >g && mv g f && patch -i ../../p0.diff && cat f")
add("patch", "a hunk that fails", W + "\n" + PATCHSET + "\ncd proj/src && printf 'q\\nr\\ns\\nt\\nu\\n' >f && patch -i ../../p0.diff; echo \"status=$?\"; ls; cat f.rej", error=True)
add("patch", "dry run", W + "\n" + PATCHSET + "\ncd proj/src && patch --dry-run -i ../../p0.diff && cat f")
add("patch", "from stdin", W + "\n" + PATCHSET + "\ncd proj/src && patch < ../../p0.diff && cat f")
add("patch", "missing patch file", W + "\n" + PATCHSET + "\npatch -i nope.diff", error=True)
add("patch", "missing target file", W + "\n" + PATCHSET + "\npatch -i p0.diff nope; echo \"status=$?\"", error=True)
add("patch", "garbage input", W + "\n" + PATCHSET + "\nprintf 'not a patch\\n' | patch proj/src/f; echo \"status=$?\"", error=True)

# -- grep -rI and cut -z over a tree --------------------------------------------------------------
BINTREE = TREE + "\nprintf 'TODO\\000bin' >src/blob.bin"
add("grep", "recursive ignoring binary files", BINTREE + "\ngrep -rI \"TODO\" . | sort")
add("grep", "recursive ignoring binary files (long)", BINTREE + "\ngrep --recursive --binary-files=without-match \"TODO\" . | sort")
add("grep", "recursive including binary files", BINTREE + "\ngrep -r \"TODO\" . | sort")
add("grep", "recursive in a directory", BINTREE + "\ngrep -rI \"TODO\" src")
add("grep", "recursive with no match", BINTREE + "\ngrep -rI \"NOTHERE\" .; echo \"status=$?\"")
add("grep", "recursive in a missing directory", BINTREE + "\ngrep -rI \"TODO\" nosuch", error=True)
add("cut", "fields of NUL-terminated find output", TREE + "\nfind . -print0 | sort -z | cut -z -d \"/\" -f 2 | tr '\\0' '\\n' | uniq")
add("cut", "fields of NUL-terminated find output (long)", TREE + "\nfind . -print0 | sort -z | cut --zero-terminated --delimiter \"/\" --fields 2 | tr '\\0' '\\n' | uniq")
add("wc", "count find output", TREE + "\nfind . | wc")


# Dropped after recording, with the reason for each group.
# GNU quotes the name with curly quotes under the oracle's UTF-8 locale and nothing else differs: the documented
# deliberate difference UTF8_QUOTES (env_facts.py, command_errors.py).
_DROPPED_Q = (
    'real tldr basename: extra operand',
    'real tldr mkdir: create an existing directory',
    'real tldr mkdir: create over a file',
    'real tldr mkdir: create under a missing parent',
    'real tldr mktemp: custom directory that is missing',
    'real tldr mktemp: template with too few Xs',
    'real tldr touch: invalid specific time',
)
_DROPPED = set(_DROPPED_Q)
CASES = [case for case in CASES if case[0] not in _DROPPED]
