"""Generate a standalone component-text fixture with embedded web assets.

No guest-language compiler is needed. Run with Python 3 after editing the assets.
The generated WAT can be connected directly or encoded as a .wasm component.
"""
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUTPUT = HERE.parent.parent / 'fixtures' / 'web-demo.wat'
segments = []
offset = 1024

def data(value):
    global offset
    value = value.encode('utf-8') if isinstance(value, str) else value
    start = offset
    segments.append(f'    (data (i32.const {start}) "' + ''.join(f'\\{b:02x}' for b in value) + '")')
    offset += len(value) + 8
    return start, len(value)

def constants(pair):
    return f'i32.const {pair[0]} i32.const {pair[1]}'

name = data('content-type')
html_type = data('text/html; charset=utf-8')
js_type = data('text/javascript; charset=utf-8')
json_type = data('application/json')
text_type = data('text/plain; charset=utf-8')
post = data('POST')
html = data((HERE / 'index.html').read_bytes())
js = data((HERE / 'app.js').read_bytes())
info = data('{"component":"web-demo","runtime":"WebAssembly","mounted":true}')
missing = data('Not found')
routes = {path: data(path) for path in ['/', '/index.html', '/app.js', '/api/info', '/api/echo', '/api/path', '/api/counter', '/api/increment', '/api/reset', '/trap', '/spin']}
heap = ((offset + 15) // 16) * 16

# The response record at address 0 has status:u16 at 0, headers:ptr/len at 4/8,
# and body:ptr/len at 12/16. Its one header tuple lives at address 64.
wat = f'''(component
  (core module $code
    (memory (export "memory") 32)
    (global $heap (mut i32) (i32.const {heap}))
    ;; This mutable guest global is shared by WIT calls and HTTP requests.
    (global $count (mut i32) (i32.const 0))
    (func $get (export "get") (result i32) global.get $count)
    (func $increment (export "increment") (param $amount i32) (result i32)
      global.get $count local.get $amount i32.add global.set $count global.get $count)
    (func $reset (export "reset") (result i32)
      i32.const 0 global.set $count global.get $count)
    ;; Called after the host has copied the response; reclaim transient request memory.
    (func (export "post-return") (param i32) i32.const {heap} global.set $heap)
    (func $render-count (result i32)
      (local $n i32) (local $cursor i32)
      global.get $count local.set $n
      i32.const 160 local.set $cursor
      loop $digits
        local.get $cursor i32.const 1 i32.sub local.tee $cursor
        local.get $n i32.const 10 i32.rem_u i32.const 48 i32.add i32.store8
        local.get $n i32.const 10 i32.div_u local.tee $n br_if $digits
      end
      i32.const 200 {constants(json_type)} local.get $cursor
      i32.const 160 local.get $cursor i32.sub call $respond)
{chr(10).join(segments)}
    (func (export "realloc") (param i32 i32 i32 i32) (result i32)
      (local $ptr i32)
      global.get $heap local.get 2 i32.const 1 i32.sub i32.add
      i32.const 0 local.get 2 i32.sub i32.and local.tee $ptr
      local.get 3 i32.add global.set $heap local.get $ptr)
    (func $equals (param $a i32) (param $alen i32) (param $b i32) (param $blen i32) (result i32)
      (local $i i32)
      local.get $alen local.get $blen i32.ne if i32.const 0 return end
      block $done loop $next
        local.get $i local.get $alen i32.ge_u br_if $done
        local.get $a local.get $i i32.add i32.load8_u
        local.get $b local.get $i i32.add i32.load8_u
        i32.ne if i32.const 0 return end
        local.get $i i32.const 1 i32.add local.set $i br $next
      end end i32.const 1)
    (func $respond (param $status i32) (param $type i32) (param $tlen i32) (param $body i32) (param $blen i32) (result i32)
      i32.const 0 local.get $status i32.store16
      i32.const 4 i32.const 64 i32.store
      i32.const 8 i32.const 1 i32.store
      i32.const 12 local.get $body i32.store
      i32.const 16 local.get $blen i32.store
      i32.const 64 i32.const {name[0]} i32.store
      i32.const 68 i32.const {name[1]} i32.store
      i32.const 72 local.get $type i32.store
      i32.const 76 local.get $tlen i32.store
      i32.const 0)
    (func (export "handle")
      (param $method i32) (param $mlen i32) (param $path i32) (param $plen i32)
      (param $headers i32) (param $hlen i32) (param $body i32) (param $blen i32) (result i32)
'''
for route, mime, body in [('/', html_type, html), ('/index.html', html_type, html), ('/app.js', js_type, js), ('/api/info', json_type, info)]:
    wat += f'''      local.get $path local.get $plen {constants(routes[route])} call $equals
      if i32.const 200 {constants(mime)} {constants(body)} call $respond return end
'''
wat += f'''      local.get $path local.get $plen {constants(routes['/api/counter'])} call $equals
      if call $render-count return end
      local.get $path local.get $plen {constants(routes['/api/increment'])} call $equals
      if
        local.get $method local.get $mlen {constants(post)} call $equals i32.eqz
        if i32.const 405 {constants(text_type)} {constants(missing)} call $respond return end
        i32.const 1 call $increment drop call $render-count return
      end
      local.get $path local.get $plen {constants(routes['/api/reset'])} call $equals
      if
        local.get $method local.get $mlen {constants(post)} call $equals i32.eqz
        if i32.const 405 {constants(text_type)} {constants(missing)} call $respond return end
        call $reset drop call $render-count return
      end
      local.get $path local.get $plen {constants(routes['/api/echo'])} call $equals
      if i32.const 200 {constants(json_type)} local.get $body local.get $blen call $respond return end
      ;; Prefix match lets this endpoint demonstrate path + query forwarding.
      local.get $plen i32.const 9 i32.ge_u
      if local.get $path i32.const 9 {constants(routes['/api/path'])} call $equals
        if i32.const 200 {constants(text_type)} local.get $path local.get $plen call $respond return end
      end
      local.get $path local.get $plen {constants(routes['/trap'])} call $equals if unreachable end
      local.get $path local.get $plen {constants(routes['/spin'])} call $equals if loop $spin br $spin end end
      i32.const 404 {constants(text_type)} {constants(missing)} call $respond)
  )
  (core instance $code (instantiate $code))
  (type $headers (list (tuple string string)))
  (type $request (record (field "method" string) (field "path" string) (field "headers" $headers) (field "body" (list u8))))
  (type $response (record (field "status" u16) (field "headers" $headers) (field "body" (list u8))))
  (func $handle (param "request" $request) (result $response)
    (canon lift (core func $code "handle") (memory $code "memory") (realloc (func $code "realloc")) (post-return (func $code "post-return"))))
  (instance $web
    (export "headers" (type $headers))
    (export "request" (type $request))
    (export "response" (type $response))
    (export "handle" (func $handle)))
  (export "agent:web/handler@0.1.0" (instance $web))
  (func $get (result u32) (canon lift (core func $code "get")))
  (func $increment (param "amount" u32) (result u32) (canon lift (core func $code "increment")))
  (func $reset (result u32) (canon lift (core func $code "reset")))
  (instance $counter
    (export "get" (func $get))
    (export "increment" (func $increment))
    (export "reset" (func $reset)))
  (export "example:counter/state@0.1.0" (instance $counter))
)
'''
OUTPUT.write_text(';; Generated by examples/web-demo/build.py; assets are embedded in this component.\n' + wat)
print(OUTPUT)
