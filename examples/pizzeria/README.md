# Forno Pizza: discover a restaurant, read its menu, order through an agent

A simulated restaurant for the Hyperlocal LAN peer workflow. The restaurant
hosts one WASM component containing its website, typed menu/order functions,
and an in-memory order ledger. A guest opens the restaurant's web interface
and calls its hosted functions; no component is downloaded to the guest.

The menu includes **Hawaii** (the demo favorite) and **Berghain**, a black-dough
pizza with smoked mozzarella, mushrooms, black garlic, and chilli honey.
The Berlin × Codex specials add Checkpoint GPT, Kreuzberg Context, Token
Currywurst, The Merge Conflict, KitKat (pink beetroot base), and about blank
(white pizza with potato, rosemary, and lemon). The menu has eight pizzas in total.
All prices and orders are fictional. No payment is taken and nothing is sent
to a real kitchen or POS.

## Try it on one computer

With `just` installed, run `just demo`. For example,
`just demo --pizza berghain --table 12` selects Berghain for table 12.

From the repository, build the server if necessary:

```sh
cargo build --locked
python3 examples/pizzeria/demo.py
```

The launcher starts separate restaurant and guest MCP processes, discovers the
named restaurant using mDNS, inspects its functions, and opens a loopback web
proxy. It prints the menu URL. Open that URL, then enter **order** in the demo
terminal. The guest calls `place-order` for one Hawaii at table 7. The receipt
appears automatically in the website's **Your table's orders** panel.

Commands:

- `menu`: read the menu through a peer function call.
- `order`: place the simulated order, or retrieve its receipt on an identical retry.
- `orders`: read the restaurant's current orders for the selected table.
- `stop`: close both demo MCP processes and their listeners.

`--pizza berghain --table 12` changes the demo order and initial menu selection.
The launcher never orders until you enter `order`. Repeating that command reuses
the same request ID and cannot create a second order. To demonstrate a separate
order, use a new request ID through your agent.

The launcher verifies the restaurant component's digest against this repository's
known simulated fixture before ordering. If multicast is unavailable, the
one-computer launcher reports a loopback fallback to its own restaurant process.
It does not configure or join a Wi-Fi network.

## Demo on two devices on the same Wi-Fi

The root `justfile` provides separate MCP stdio launchers:

```sh
just mcp restaurant                 # Forno Pizza MCP server, LAN port 47831
just mcp restaurant --name "My Pizzeria" --port 47832
just mcp guest                      # Plain guest MCP stdio server
just run guest                      # Codex CLI with the guest MCP server
just run restaurant                 # Codex CLI hosting Forno Pizza
```

Use `just mcp restaurant` on the restaurant computer, then run `just run guest` in
another terminal on the guest computer. It builds the server and opens Codex CLI
with a session-only `hyperlocal_guest` MCP configuration. Codex CLI must already
be installed and on PATH. Your saved Codex configuration is unchanged; no prompt
or order is sent automatically. Use `/mcp` in Codex to check the connection,
then type the restaurant discovery and ordering request below yourself.

To use an agent on both sides, run `just run restaurant` **instead of**
`just mcp restaurant`, then run `just run guest` in a separate terminal. The
restaurant session starts its own `hyperlocal_restaurant` MCP server on LAN
port 47831 with the component connected as `restaurant`. Do not run both
restaurant launchers at once on that port: separate processes own separate
order ledgers. Keep the restaurant Codex session open while guests order.

In the restaurant agent, ask:

```text
Use the local restaurant plugin to show the current orders for table 7 via
get-table-orders. Include order IDs, pizza names, quantities, and status.
```

Ask again after a guest orders to read the updated ledger. These are on-demand
reads; the launcher does not subscribe to orders or start automatic polling.
The restaurant agent, guest peer calls, and menu all use the same live instance.
The restaurant agent can also call `get-open-orders` to read orders across all
tables. In a browser, open `/back-office` on the menu's origin (or use the menu
footer link) for an automatically refreshing view of every received order.
The demo has no staff authentication: this back office is available to anyone
who can access the hosted component. Received orders remain open; completion
and cancellation are not implemented.

Extra arguments go to Codex, for example `just run guest --no-alt-screen`. The guest
uses `.hyperlocal/guest-plugins` as its download cache and does not advertise
itself on the LAN. For another harness, use `just mcp guest` as its MCP stdio
command with this repository as its working directory; extra arguments go to
the MCP server. The restaurant stays available while its terminal remains open.
Use `just demo` for the interactive terminal flow that drives both servers.

On the restaurant's computer:

```sh
./target/debug/hyperlocal-agent-apps \
  --share-live --name "Forno Pizza" \
  --plugin restaurant=fixtures/pizzeria.wasm
```

Use these arguments in its MCP harness configuration, or keep the process open
in a terminal for the demo. The restaurant advertises its name through mDNS.
Its menu and functions share the same retained instance.

On the guest's computer, join the same Wi-Fi, run a Hyperlocal-enabled coding
harness, and ask:

```text
Find the local peer named Forno Pizza. Inspect its restaurant plugin and open
its hosted menu for table 7. Read the menu and place a simulated order for one
Hawaii using place-order with a unique request ID. Show the receipt. Do not
download or reconnect the restaurant component.
```

The guest flow uses `discover_peers`, `list_peer_plugins`, `inspect_peer_plugin`,
`open_peer_web`, and `call_peer_plugin`. The peer name appears in the tool results
and menu header. The restaurant's own agent can read the same order using
`get-order` or `get-table-orders`. The UI polls every 1.5 seconds.

A guest Wi-Fi with client isolation or blocked multicast may prevent discovery
or peer connections. A known restaurant LAN address can substitute for discovery
when direct device-to-device HTTP is allowed.

## Typed API

Interface: `restaurant:pizzeria/service@0.1.0`.

| Function | Arguments | Result |
| --- | --- | --- |
| `get-menu` | none | Restaurant name, currency, and eight pizzas with prices in cents |
| `place-order` | `pizza-id`, `quantity`, `table`, `request-id` | `result<order, order-error>` |
| `get-order` | `id` | `option<order>` |
| `get-table-orders` | `table` | `result<list<order>, order-error>` |
| `get-open-orders` | none | All received orders across tables |

After remote inspection, call `call_peer_plugin` with the digest returned there:

```json
{
  "peer_url": "http://192.168.1.20:47831/",
  "plugin_id": "restaurant",
  "sha256": "<digest from inspect_peer_plugin>",
  "function": ["restaurant:pizzeria/service@0.1.0", "place-order"],
  "arguments": {
    "pizza-id": "hawaii",
    "quantity": 1,
    "table": 7,
    "request-id": "table-7-hawaii-demo-001"
  }
}
```

Prices are calculated inside the restaurant component. Hawaii is EUR 13.50;
Berghain is EUR 16.50. Orders accept tables 1–40 and quantities 1–5. Request IDs
must be 8–64 ASCII letters/digits or `._-`. Reusing an ID with identical arguments
returns the original receipt; different arguments return `request-id-conflict`.
The guest returns typed errors for validation failures without faulting its instance.

The website serves `/`, `/style.css`, `/app.js`, `/pizza.svg`, `/api/menu`,
`/api/orders?table=7`, and `/api/orders/<id>`. The **Place demo order** button
posts JSON to `/api/orders` with `pizza_id`, `quantity`, `table`, and `request_id`.
This calls the same validated, idempotent order function as agents. Retries reuse
the request ID; after success, **Place another demo order** explicitly starts a
new order. Selecting a pizza or copying an agent request does not place an order.
`/back-office` and `/back-office.js` provide the restaurant view, polling
`/api/open-orders` every 1.5 seconds while visible. Both views are embedded in
the component and work through the remote peer web proxy.

The ledger holds up to 256 orders in the restaurant's guest memory. Order IDs
and idempotency records reset on plugin reconnection or server restart. There
is no disk persistence, payment, authentication, or real kitchen integration.
The general server's `--share-live` access model applies.

## Rebuild the component

The checked-in `fixtures/pizzeria.wasm` runs directly. Rebuilding requires Rust's
`wasm32-unknown-unknown` target and Python 3:

```sh
rustup target add wasm32-unknown-unknown
python3 examples/pizzeria/build.py
```

The Rust guest uses `wit-bindgen` to export its typed WIT functions and the shared
`agent:web/handler@0.1.0` interface. The build script embeds the local web assets,
then uses the host's `package_component` example to wrap the core module as a
standalone component. The component has no host/WASI imports and uses normal
binding-generated memory cleanup.

```sh
cargo test --locked --test pizzeria
cargo clippy --locked --manifest-path examples/pizzeria/plugin/Cargo.toml \
  --release --target wasm32-unknown-unknown -- -D warnings
```
