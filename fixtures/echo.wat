;; Minimal canonical ABI fixture for values that pass through guest linear memory.
(component
  (core module $code
    (memory (export "memory") 1)
    (global $heap (mut i32) (i32.const 1024))
    ;; Test-only bump allocator; inputs in these tests fit within one page.
    (func (export "realloc") (param i32 i32 i32 i32) (result i32)
      (local $ptr i32)
      global.get $heap
      local.get 2 i32.const 1 i32.sub i32.add
      i32.const 0 local.get 2 i32.sub i32.and
      local.tee $ptr
      local.get 3 i32.add global.set $heap
      local.get $ptr)
    (func (export "post-return") (param i32) i32.const 1024 global.set $heap)
    (func (export "echo") (param i32 i32) (result i32)
      i32.const 0 local.get 0 i32.store
      i32.const 4 local.get 1 i32.store
      i32.const 0)
  )
  (core instance $code (instantiate $code))
  (func (export "echo") (param "text" string) (result string)
    (canon lift (core func $code "echo")
      (memory $code "memory") (realloc (func $code "realloc")) (post-return (func $code "post-return"))))
  (func (export "echo-bytes") (param "bytes" (list u8)) (result (list u8))
    (canon lift (core func $code "echo")
      (memory $code "memory") (realloc (func $code "realloc")) (post-return (func $code "post-return"))))
)
