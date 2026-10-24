# Hyperlocal Agent Apps

A minimal Rust MCP server that loads WebAssembly components while running,
reads their WIT interfaces, and dynamically invokes their exported functions.
No plugin manifest, generated host bindings, or server restart is required.

## Run

Requires Rust 1.93 or newer.

```sh
cargo build --locked
./target/debug/hyperlocal-agent-apps
```

The server speaks newline-delimited MCP JSON-RPC on stdin/stdout. It starts
with an empty, in-memory plugin registry. Relative component paths resolve
against the server's working directory. stdout is reserved for MCP messages.

Configure your harness with its MCP server settings, for example:

```json
{
  "mcpServers": {
    "hyperlocal": {
      "command": "/absolute/path/to/hyperlocal-agent-apps/target/debug/hyperlocal-agent-apps"
    }
  }
}
```

For an optimized binary, use `cargo build --release --locked` and point the
harness at `target/release/hyperlocal-agent-apps`.

## Tools

For ChatGPT plugin connections, `just chatgpt` starts a loopback Streamable HTTP
MCP endpoint at `http://127.0.0.1:47832/mcp`. See the
[ChatGPT connection guide](examples/pizzeria/CHATGPT.md) for Secure MCP Tunnel
and private plugin registration. This is separate from Codex's stdio settings.

The eleven MCP tools stay fixed as components are connected:

| Tool | Arguments | Result |
| --- | --- | --- |
| `connect_plugin` | `id`, `path` | Plugin id, function count, and optional `web_url` |
| `list_plugins` | none | Connected ids, paths, function counts, and web URLs |
| `inspect_plugin` | `id` | Reconstructed WIT and typed function descriptions |
| `call_plugin` | `id`, `function`, `arguments` | A `results` array |
| `discover_peers` | optional `wait_ms` (100–10000, default 2000) | IPv4 LAN peer URLs and mDNS diagnostics |
| `list_peer_plugins` | `peer_url` | Catalog with ids, sizes, and SHA-256 digests |
| `download_plugin` | `peer_url`, `plugin_id`, `sha256` | Verified local component path; nothing is instantiated |
| `inspect_peer_plugin` | `peer_url`, `plugin_id` | Named peer's WIT, function paths, and component digest |
| `call_peer_plugin` | `peer_url`, `plugin_id`, `sha256`, `function`, `arguments` | Named peer and hosted function result |
| `open_peer_web` | `peer_url`, `plugin_id` | Named connection and local proxy `web_url` |
| `close_peer_web` | `web_url` | Closes your proxy; leaves the hosted plugin running |

Connect a component using an arbitrary local id:

```json
{"id":"math","path":"/absolute/path/to/fixtures/calculator.wat"}
```

Inspect it:

```json
{"id":"math"}
```

Invoke its interface export:

```json
{
  "id": "math",
  "function": ["example:math/calculator@0.1.0", "add"],
  "arguments": {"a": 20, "b": 22}
}
```

Result:

```json
{"results":[42]}
```

Function paths are arrays of exact export names, so interface names containing
slashes and versions are unambiguous. Root functions use a one-element array.
Named arguments must match the WIT parameters exactly.

Connect another file at any time. Reconnect an existing id to atomically replace
its component; compilation or validation failures preserve the previous version.
Calls already in flight retain their original component. Connections are explicit:
the server does not watch directories or automatically reload changed files.

## LAN peers and component downloads

Discovery uses mDNS/DNS-SD service type `_hyperlocal._tcp.local.`. The default
server does not advertise or expose a LAN listener. Discovery runs only when
`discover_peers` is called.

On a machine that should share plugins, start the server with `--share`:

```sh
./target/debug/hyperlocal-agent-apps \
  --share --name "Studio Mac" \
  --plugin counter=fixtures/web-demo.wat
```

Keep this process attached to your MCP harness. `--plugin id=path` is a repeatable
startup preload; components connected later through MCP appear in the catalog
automatically. `--port 47831` fixes the sharing port if needed; otherwise an
available port is chosen and printed to stderr. `--help` lists all options.
The same flags can be added to your harness's MCP command arguments.

With `--share`, **all currently connected component binaries are readable by
other devices that can reach the sharing port**. The binary includes any assets
or static data embedded by its author. With plain `--share`, the service exposes only the component
catalog and original binaries, not local paths, MCP operations, browser APIs,
credentials, or live WASM memory. Live interfaces require the separate
`--share-live` opt-in described below. Browser listeners remain on loopback. Normal
MCP startup without `--share` keeps sharing disabled.

On the receiving machine, call these MCP tools in order:

1. `discover_peers` with `{"wait_ms":3000}`. Select one of the returned `urls`.
2. `list_peer_plugins` with `{"peer_url":"http://192.168.1.20:47831/"}`.
3. Select an id and digest from the catalog, then call:

   ```json
   {
     "peer_url": "http://192.168.1.20:47831/",
     "plugin_id": "counter",
     "sha256": "<64-character lowercase digest from the catalog>"
   }
   ```

   Use `download_plugin` for this step. It returns an absolute local `path`.
4. Call `connect_plugin` with a chosen local `id` and the downloaded `path`.
   This explicitly instantiates your own copy, including its local browser mount.

A suitable agent prompt is:

```text
Discover Hyperlocal peers on my network and list their shared plugins.
Download the counter component from the peer I select, then connect it locally
as peer-counter and show its web_url.
```

Downloads go to `.hyperlocal/plugins/<sha256>.wasm` relative to the server's
working directory. Use `--download-dir /absolute/path` to choose a stable cache.
The directory is created only after a download has passed validation. Writes
are atomic, existing valid files are reused, and corrupt cache entries are
reported rather than overwritten. A peer changing its advertised version
requires selecting the new digest explicitly. Source `.wat` files are converted
to binary components; sharing serves the exact bytes retained when connected,
even if the original source file is subsequently edited or removed.

Downloads verify the catalog's size and SHA-256, require a valid binary component,
and never run guest code (including start functions). Connecting starts at the
component's initial state: downloading another machine's counter does not copy
its current count. Peer names, metadata, and mDNS announcements are untrusted;
the checksum checks byte integrity against the chosen catalog, not publisher
identity. This first version uses unauthenticated HTTP on the LAN and has no
signatures or account pairing.

The discovery and download client currently support numeric IPv4 addresses on
private, link-local, or loopback networks. Client requests bypass HTTP proxies,
refuse redirects, have bounded timeouts, and limit catalogs to 256 KiB / 256
entries and components to 32 MiB. Multicast may be unavailable on guest Wi-Fi,
VPNs, or networks with client isolation. In that case you can use a known LAN
IP and sharing port directly with `list_peer_plugins`. Discovery does not cross
routers or connect to a public registry.

The distribution HTTP API is versioned separately from plugin WIT:

- `GET /v1/catalog` returns protocol version 1, the advertised peer name, component
  metadata, and `web_available` / `calls_available` flags.
- `GET /v1/components/<sha256>` returns immutable binary component bytes.

This metadata is generated by the host. Plugin authors still need no manifest.

## Connect to a peer's live interfaces

Use this workflow when you want the peer's current state. Downloads create a
separate local instance; remote calls and web proxies use the peer's existing
instance, shared with its own agent and browser.

The publishing machine must explicitly opt in:

```sh
./target/debug/hyperlocal-agent-apps \
  --share-live --name "Studio Mac" \
  --plugin counter=fixtures/web-demo.wat
```

`--share-live` implies binary sharing and additionally exposes **all connected
plugins' hosted functions and web interfaces to reachable LAN clients**. Plain
`--share` keeps these endpoints disabled. This v0.1 LAN service is unauthenticated;
only enable live sharing for peers you intend to let read and change that state.
There is no remote connect/install/reset-runtime administration API.

Peers announce `--name` in both mDNS and the catalog. Discovery returns the name
alongside addresses. Remote inspection, function results, and web connection
results also include `peer_name`, so the agent can identify **Studio Mac / counter**.
Names are display labels, not authenticated identities; addresses distinguish
peers with duplicate names.

On the receiving machine:

1. `discover_peers` and select the named peer.
2. `list_peer_plugins` shows which plugins have live calls or web access enabled.
3. `inspect_peer_plugin` with `peer_url` and `plugin_id` returns WIT, exact function
   paths, the current `sha256`, and the peer name.
4. `call_peer_plugin` invokes a function on that same hosted instance:

   ```json
   {
     "peer_url": "http://192.168.1.20:47831/",
     "plugin_id": "counter",
     "sha256": "<digest from inspect_peer_plugin>",
     "function": ["example:counter/state@0.1.0", "increment"],
     "arguments": {"amount": 41}
   }
   ```

   The result contains `peer_name`, `plugin_id`, `sha256`, and
   `result: {"results":[41]}`.
5. `open_peer_web` with the same peer and plugin returns a named connection and
   a local loopback `web_url`. Open it in the browser. Clicking **Add 1 from
   browser** changes the remote counter to 42. A remote `get` then returns 42.

For example, ask your agent:

```text
Discover Hyperlocal peers and connect to Studio Mac's hosted counter.
Inspect its functions, open its web UI, and increment the remote counter by 41.
Use its current state; do not download or reconnect the component.
```

The proxy preserves root-relative paths and queries, so `/app.js` and browser
`fetch('/api/...')` work. It creates no local component instance and writes no
download files. Calling `open_peer_web` again for the same peer, plugin and
digest reuses the proxy. `close_peer_web` closes only your local proxy. The remote
instance continues running. All proxies close when your receiving server exits.

Web proxies expose `GET /.hyperlocal/connection` locally, containing the peer
name, address, plugin id and digest. The demo uses this to show the peer name in
its badge and browser tab title. Other plugins may use it as optional UI context.
This path is reserved on peer proxies; ordinary local plugins own their own paths.

Calls and proxies pin the inspected component digest. If the publisher replaces
a plugin with different code, old calls/proxies fail with a version-change error.
Inspect again and reopen the proxy to accept the replacement. Reconnecting the
same component on the publisher can still reset its state without changing its
digest. A failed remote request is not a transaction rollback; mutations may have
completed before a timeout or dropped response. Automatic retries are disabled.

The machine gateway rejects browser Origin/Fetch-Metadata headers and requires
JSON POSTs. Browser access uses the origin-checked loopback proxy, which strips
ambient cookies, Authorization, Origin, Referer and transport headers before
forwarding; remote Set-Cookie responses are dropped. Remote HTML/JavaScript is
still untrusted web content. UI responses remain buffered, with no WebSockets or
streaming. Remote function requests are limited to 1 MiB and replies to 8 MiB;
web requests retain the existing 1 MiB input / 8 MiB output body limits.

The additional HTTP endpoints exist only with `--share-live`:

- `GET /v1/plugins/<id>`: WIT and function metadata, without local filesystem paths.
- `POST /v1/plugins/<id>/call`: pinned function path and named arguments.
- `POST /v1/plugins/<id>/web`: pinned WIT request/response envelope, used by proxies.

## Plugin contract

A plugin is a Component Model `.wasm` binary with embedded interface types.
Component text (`.wat`) is also accepted for easy experimentation. Core Wasm
modules alone are insufficient. No adjacent `.wit` or TOML file is read at runtime.

`inspect_plugin` reconstructs WIT from the component and returns function paths,
parameter names, type descriptions, results, and a `callable` flag. This does not
recover source comments or examples absent from the component. Functions inside
exported interfaces and root exported functions are both supported.

For example, the calculator fixture exposes this interface:

```wit
package example:math@0.1.0;

interface calculator {
    record pair {
        first: u32,
        second: u32,
    }
    add: func(a: u32, b: u32) -> u32;
    second: func(pair: pair) -> u32;
}
```

The readable fixture in `fixtures/calculator.wat` needs no guest-language
compiler. The tests also turn it into a binary component and connect that binary.
For your own plugins, build with a Component Model toolchain and export your
application's typed WIT functions directly.

## Browser interfaces

A component can also export `agent:web/handler@0.1.0` with the `handle` function
specified in [`wit/web.wit`](wit/web.wit). This is an optional WIT convention;
no manifest or host import is needed.

```wit
handle: func(request: request) -> response;
```

Requests contain `method`, `path` (including the query), `headers`, and a byte
`body`. Responses contain `status`, `headers`, and a byte `body`. Return HTML,
JavaScript, CSS, images, or API responses with the appropriate `content-type`.
The component owns routing and embeds or generates its assets. The host never
serves files from the plugin's directory.

As soon as `connect_plugin` succeeds, a web-capable component is mounted on its
own `http://127.0.0.1:<port>/` origin. The result includes its actual `web_url`;
`list_plugins` and `inspect_plugin` expose it too. Root-relative asset paths and
browser `fetch('/api/...')` work normally. Ordinary components return `null` for
`web_url` and do not open listeners.

Try the self-contained demo:

```json
{"id":"web-demo","path":"/absolute/path/to/fixtures/web-demo.wat"}
```

Open the returned `web_url` in your browser. The component serves a page at `/`,
JavaScript at `/app.js`, and a shared counter through `/api/counter`,
`/api/increment`, and `/api/reset`. It also retains `/api/info` and `/api/echo`
for basic HTTP experiments.
The editable assets live in `examples/web-demo/`; run
`python3 examples/web-demo/build.py` to regenerate the fixture after editing them.
Python is only needed to rebuild the fixture, not to run it.

The mount remains available between MCP calls for the life of the MCP server.
Replacing a web plugin with another web plugin under the same id preserves its
URL and serves the new component. A failed connection leaves the existing mount
intact. Replacing it with a component without the web export closes its listener.
Stopping the MCP server closes all mounts. A new server session gets new ports.

## Pizza restaurant demo

With `just` installed, use `just mcp restaurant` to launch the restaurant and
`just run guest` to open Codex CLI with the guest MCP server configured for that
session. `just mcp guest` provides the plain guest MCP stdio server for other
harnesses; `just demo` runs the interactive two-process demo.
Use `just run restaurant` instead of `just mcp restaurant` to host the restaurant
inside a Codex CLI session whose agent can read incoming orders from that same
live component.

Run `cargo build --locked` followed by `python3 examples/pizzeria/demo.py` to
start a named “Forno Pizza” restaurant peer and a separate guest MCP server.
Open the printed menu URL, then type `order` in the launcher to place a
simulated Hawaii order through the peer function. Its receipt appears in the
browser using the same live WASM state. The menu also includes Berghain, a
black-dough pizza. No payment or real kitchen dispatch takes place.

See [the pizzeria guide](examples/pizzeria/README.md) for a two-device Wi-Fi demo,
the WIT API, and component build instructions.

## Shared agent and browser state

Each connected plugin owns one live Wasmtime store and component instance.
MCP calls and HTTP requests acquire the same per-plugin lock before invoking
exports. Guest globals and linear memory survive calls and page reloads. Calls
to different plugins remain independent; calls to the same plugin run one at a
time. The state is in the WASM guest, not copied into a host-side counter.

The demo exports [`example:counter/state@0.1.0`](examples/web-demo/counter.wit)
alongside its web handler:

```wit
get: func() -> u32;
increment: func(amount: u32) -> u32;
reset: func() -> u32;
```

To demonstrate both directions:

1. Connect `fixtures/web-demo.wat` as `counter` and open its `web_url`.
2. Call `call_plugin` with:

   ```json
   {
     "id": "counter",
     "function": ["example:counter/state@0.1.0", "increment"],
     "arguments": {"amount": 41}
   }
   ```

3. The page displays **41** automatically (it polls once per second).
4. Click **Add 1 from browser**. The page displays **42**.
5. Call the same interface's `get` with empty arguments `{}`. It returns
   `{"results":[42]}`. Browser **Reset** and the typed `reset` function both
   reset the same counter to zero. The counter uses u32 wrapping arithmetic.

For the installed Codex MCP server, restart your CLI session to load the rebuilt
binary, then ask:

```text
Connect fixtures/web-demo.wat as counter using hyperlocal. Show its web_url,
inspect its counter interface, and increment the counter by 41. After I change
it in the browser, read the counter again without reconnecting the plugin.
```

Connecting compiles, validates, and instantiates the new component before it
replaces the old one. Reconnecting under the same id deliberately resets guest
state, while preserving a web-to-web replacement's URL. A failed replacement
preserves both the old state and mount. Already admitted calls finish on their
original instance. State does not survive a server restart and is not saved to disk.

Fuel is refilled for every call; memory limits apply to the retained instance.
Guest allocators must free transient allocations. The demo uses a canonical
post-return hook to reclaim HTTP request memory without resetting the counter.

A guest trap, fuel exhaustion, or another failure during `Func::call` marks only
that plugin instance as faulted. Later calls report that reconnection is required;
the host never silently reinitializes it. Reconnect explicitly to recover and
reset state. Invalid arguments rejected before execution do not fault the plugin,
and a normal WIT `result::err` remains an ordinary return value. Guest mutations
before a failure are not rolled back.

Listeners bind only to loopback. Host and browser-origin checks reject unrelated
origins; each plugin has its own origin, although loopback ports are not a cookie
isolation boundary. Request bodies are limited to 1 MiB and buffered for at most
10 seconds; response bodies are limited to 8 MiB. Headers are UTF-8 strings.
Transport-framing and CORS response headers are host-controlled. Requests and
responses are buffered; this interface does not support streaming or WebSockets.
This custom interface is not `wasi:http/incoming-handler`.

## JSON value mapping

| WIT value | JSON encoding |
| --- | --- |
| `bool`, small integers, finite floats, `string` | Corresponding JSON scalar |
| `s64`, `u64` | Decimal strings on output; strings or integer JSON numbers on input |
| `char` | String containing one Unicode scalar |
| `list<T>`, tuple | Array |
| record | Object with exactly its declared fields |
| enum | Case name string |
| flags | Array of unique flag names |
| `option<T>` | `{"none":null}` or `{"some":value}` |
| result | `{"ok":value}` or `{"err":value}`; absent payload is `null` |
| variant | `{"case-name":value}`; absent payload is `null` |

For example, `some(none)` becomes `{"some":{"none":null}}`. Lists of bytes
are integer arrays. Non-finite float results produce a tool error rather than
silently becoming JSON null. Use strings when sending 64-bit integers from a
JavaScript client to avoid rounding before the request reaches the server.

## v0.1 boundaries

- Self-contained components only. All host imports, including WASI imports,
  are rejected on connection with their names in the error. No outbound HTTP, filesystem,
  credentials, sockets, or process bindings are installed.
- Each plugin retains one live instance shared by MCP and HTTP. State is in-memory only.
- Resources, maps, futures, streams, and error-context values are not callable.
  Functions using them remain discoverable with `callable: false`.
- Each call gets 10 million fuel units. Each retained instance has a 64 MiB
  limit per guest linear memory, at most 10 memories, and bounded tables/instances.
  Instantiation has its own 10 million fuel unit budget. Component input files are limited to 32 MiB. These limits
  do not bound compilation time, host-side JSON allocation, or total process memory.
- No JavaScript orchestration tool, host capability API, plugin dependency linking,
  disk persistence of guest state, signatures, pairing, or public registries yet.

Loading, inspecting, and calling run on blocking workers; registry locks are only
held during lookup or publication. MCP errors from plugin operations are returned
as tool results with `isError: true`, leaving the server available for further calls.

## Verify

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check
```

Tests cover dynamic interface discovery, binary and text components, named record
arguments, Unicode strings and byte lists, memory limits, connecting multiple
plugins, atomic replacement, invalid arguments, unbound imports, traps, fuel
exhaustion, composite value conversion, and a real stdio MCP session. HTTP tests
cover mounted assets and APIs, POST bodies, queries, separate origins, hot
replacement, origin checks, explicit recovery after traps, and listener cleanup.
Shared-state tests mix MCP and HTTP updates, verify concurrent increments, refill
fuel without losing globals, reclaim transient HTTP memory, and check that
reconnection resets state while failed replacement preserves it. Tests need
permission to bind and connect to local sockets. Peer tests cover immutable byte
transfer, digest/version failures, corrupt caches, non-component rejection,
explicit execution, and a full MCP download/connect flow. Live-peer tests verify
named inspection and results, remote/browser shared state, default-off access,
version pinning, browser gateway rejection, credential stripping, and proxy cleanup. The actual multicast
test is opt-in because it needs a suitable LAN interface:

```sh
cargo test --locked --test peers mdns_discovers_a_live_advertised_peer -- --ignored
```
