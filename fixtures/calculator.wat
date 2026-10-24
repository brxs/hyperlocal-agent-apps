;; This is a real component, written in text format to keep the fixture build-free.
;; connect_plugin accepts both .wasm binaries and .wat component text.
(component
  (core module $code
    (func (export "add") (param i32 i32) (result i32)
      local.get 0 local.get 1 i32.add)
    (func (export "swap") (param i32 i32) (result i32)
      local.get 1)
    (func (export "spin") (loop $forever br $forever))
    (func (export "trap") unreachable)
  )
  (core instance $code (instantiate $code))
  (func $add (param "a" u32) (param "b" u32) (result u32)
    (canon lift (core func $code "add")))
  (type $pair (record (field "first" u32) (field "second" u32)))
  (func $second (param "pair" $pair) (result u32)
    (canon lift (core func $code "swap")))
  (func $spin (canon lift (core func $code "spin")))
  (func $trap (canon lift (core func $code "trap")))
  (instance $calculator
    (export "pair" (type $pair))
    (export "add" (func $add))
    (export "second" (func $second))
  )
  (export "example:math/calculator@0.1.0" (instance $calculator))
  (export "spin" (func $spin))
  (export "trap" (func $trap))
)
