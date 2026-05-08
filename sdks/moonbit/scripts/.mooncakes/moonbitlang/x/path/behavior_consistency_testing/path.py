import os.path as path 

print(f'path.normpath(\'//\'): \'{path.normpath("//")}\'')
# path.normpath('//'): '//'

print(f'path.dirname("a/b/"): \'{path.dirname("a/b/")}\'')
# path.dirname("a/b/"): 'a/b'

print(f'path.dirname("a/b//"): \'{path.dirname("a/b//")}\'')
# path.dirname("a/b//"): 'a/b'

print(f'path.basename("a/b/"): \'{path.basename("a/b/")}\'') 
# path.basename("a/b/"): ''

print("# Python os.path without extname function")

print(f'path.relpath("a","../.."): \'{path.relpath("a","../..")}\'')
# this API depend on cwd absolute path 

print(f'path.join("a","/b"): \'{path.join("a","/b")}\'')