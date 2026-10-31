;; A component written directly in WAT because no Golem SDK compiles to the wasm GC and
;; exception-handling proposals yet. It is statically composed into the Rust agent, which imports
;; `golem-it:gc-probe/heap`.
(component
  (core module $heap
    (type $bytes (array (mut i8)))
    (type $node (struct (field (ref null $node)) (field (ref $bytes))))
    (global $head (mut (ref null $node)) (ref.null $node))
    (global $retained_kib (mut i32) (i32.const 0))
    (tag $thrown (param i32))

    ;; Allocates a GC array of `kib` KiB and keeps it reachable until `release`.
    (func (export "retain") (param $kib i32) (result i32)
      (global.set $head
        (struct.new $node
          (global.get $head)
          (array.new_default $bytes (i32.mul (local.get $kib) (i32.const 1024)))))
      (global.set $retained_kib (i32.add (global.get $retained_kib) (local.get $kib)))
      (global.get $retained_kib))

    (func (export "release")
      (global.set $head (ref.null $node))
      (global.set $retained_kib (i32.const 0)))

    (func (export "retained-kib") (result i32)
      (global.get $retained_kib))

    ;; Throws `value + 1` and catches it inside the module.
    (func (export "catch-thrown") (param $value i32) (result i32)
      (block $caught (result i32)
        (try_table (catch $thrown $caught)
          (throw $thrown (i32.add (local.get $value) (i32.const 1))))
        (i32.const 0)))

    (func (export "throw-uncaught")
      (throw $thrown (i32.const 0))))

  (core instance $heap_instance (instantiate $heap))

  (func $retain (param "kib" u32) (result u32)
    (canon lift (core func $heap_instance "retain")))
  (func $release
    (canon lift (core func $heap_instance "release")))
  (func $retained_kib (result u32)
    (canon lift (core func $heap_instance "retained-kib")))
  (func $catch_thrown (param "value" u32) (result u32)
    (canon lift (core func $heap_instance "catch-thrown")))
  (func $throw_uncaught
    (canon lift (core func $heap_instance "throw-uncaught")))

  (instance $heap_exports
    (export "retain" (func $retain))
    (export "release" (func $release))
    (export "retained-kib" (func $retained_kib))
    (export "catch-thrown" (func $catch_thrown))
    (export "throw-uncaught" (func $throw_uncaught)))

  (export "golem-it:gc-probe/heap" (instance $heap_exports)))
