;; A synthetic post-link experiment, unrelated to the SDK's WIT worlds.
;; The component deliberately exposes only run. Binaryen still treats every
;; core export as live, including live-export, and must preserve the start effect.
(component
  (core module $m
    (global $state (mut i32) (i32.const 0))
    (func $ctor
      i32.const 41
      global.set $state)
    (start $ctor)
    (func $helper (param $n i32) (result i32)
      (local $sum i32)
      (loop $again
        local.get $sum
        local.get $n
        i32.add
        local.set $sum
        local.get $n
        i32.const 1
        i32.sub
        local.tee $n
        br_if $again)
      local.get $sum)
    (func (export "live-export") (param i32) (result i32)
      local.get 0
      call $helper)
    (func $unreachable (result i32)
      i32.const 99)
    (func (export "run") (result i32)
      global.get $state
      i32.const 1
      i32.add))
  (core instance $i (instantiate $m))
  (func (export "run") (result u32) (canon lift (core func $i "run"))))
