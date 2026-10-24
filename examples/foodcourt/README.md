# Hypothetical Berlin food court

Four vendors share a Wi-Fi network. Each runs a separate MCP process, publishes
its own name through mDNS, and hosts a self-contained WASM component. Every
vendor has its own website and order ledger. Forno Pizza can run alongside them.
The shoe-store example remains available separately and is not part of this demo.

| Peer | Role | Port | Menu examples |
| --- | --- | --- | --- |
| Kiez Tacos | `tacos` | 47834 | Al pastor, mushroom tacos, street corn, hibiscus agua fresca |
| Mitte Ramen | `ramen` | 47835 | Miso ramen, vegan tantan, gyoza, yuzu soda |
| Spree Spritz | `spritz` | 47836 | Spritz, negroni, rhubarb fizz 0.0, citrus tonic 0.0 |
| Kreuzberg Taproom | `taproom` | 47837 | Pils, IPA, lager 0.0, apple schorle, chicken nuggets |

All menus, prices, and orders are fictional. The bars label alcoholic drinks
and include alcohol-free options. No real food or alcohol is sold, no payment
is taken, and no order reaches a kitchen or bar.

## One-command demo

```sh
just foodcourt
```

This builds the host, starts all four publishers, and prints a menu and back-office
URL for each. It does not start Forno Pizza or place orders. Enter `stop` or
press Ctrl-C to close all four processes started by this launcher. If startup
fails, it closes only the processes it started. Ports must be free.

In a separate terminal, run `just run guest`, or use your configured Hyperlocal
MCP in Codex, and ask:

```text
Discover the food-court peers. Compare the menus from Kiez Tacos, Mitte Ramen,
Spree Spritz, and Kreuzberg Taproom. Show vegetarian food and alcohol-free
drinks. Do not place any orders yet.
```

Then ask for a simulated order at one vendor. Inspect its interface first, use
that component's digest, and keep the peer URL tied to the selected name. The
same table number works across vendors, but order IDs and request IDs belong
to each vendor's own ledger. There is no combined checkout or cross-vendor
transaction. Websites display only their own vendor's orders.

## Individual terminals or vendor agents

```sh
just mcp tacos
just mcp ramen
just mcp spritz
just mcp taproom
```

Run each command in a separate terminal. To host a vendor inside Codex instead,
use `just run tacos`, `just run ramen`, `just run spritz`, or `just run taproom`.
Use one launcher per vendor/port. Extra `mcp` arguments go to the server, while
extra `run` arguments go to Codex. The older Forno Pizza commands still work.

Use `open_peer_web` with the vendor's discovered address and `plugin_id="vendor"`
to obtain a guest browser URL. A publisher's base API address is not its menu.
Append `/back-office` to the returned web origin to see all that vendor's orders.

## WIT API and component builds

All four export `foodcourt:vendor/service@0.1.0` alongside `agent:web/handler@0.1.0`:

- `get-menu()` returns the vendor name, category, tagline, currency, and items.
- `place-order(item-id, quantity, table, request-id)` returns a typed result.
- `get-order(id)`, `get-table-orders(table)`, and `get-open-orders()` read the ledger.

The validated, idempotent ordering rules match the pizzeria: tables 1–40,
quantities 1–5, request IDs 8–64 ASCII letters/digits/`._-`, and 256 orders per
vendor. Identical retries return the original order. State resets on reconnect
or restart. Browser POST requests use snake_case fields and the same functions.
There is no authentication or fulfilment workflow in the demo.

The four ready-to-run fixtures live at `fixtures/foodcourt-*.wasm`. To rebuild:

```sh
rustup target add wasm32-unknown-unknown
python3 examples/foodcourt/build.py
cargo test --locked --test foodcourt
```

One guest crate embeds each vendor's JSON menu and shared web assets at build
time using `FOODCOURT_VENDOR`. The JSON files contain menu content, not runtime
plugin manifests. Each compiled component is self-contained. WIT remains the
only runtime interface description, and the components import no host network
or filesystem capabilities.
