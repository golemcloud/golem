echo "dirname \"//home/user_name\": '$(dirname "//home/user_name")'"
# dirname "//home": '/' 
# bash shell without normalize function, here used dirname instead.
# POSIX implementation-defined manner for `//`
# > If a pathname begins with two successive <slash> characters, 
# > the first component following the leading <slash> characters may be interpreted in an implementation-defined manner
#

echo "dirname \"a/b/\": '$(dirname "a/b/")'"
# dirname "a/b/": 'a'
    
echo "basename \"a/b/\": '$(basename "a/b/")'"
# basename "a/b/": 'b'