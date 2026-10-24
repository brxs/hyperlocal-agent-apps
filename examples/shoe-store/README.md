# Kiez Kicks: a second local business peer

A simulated Berlin sneaker shop with a website, live stock by EU size, and
pickup reservations in one retained WASM component. No payments, real inventory,
or purchase commitments. The pickup desk is shared demo data with no staff login.

Start alongside Forno Pizza in separate terminals:

```sh
just mcp restaurant   # Forno Pizza, port 47831
just mcp shoes        # Kiez Kicks, port 47833
just run guest       # Codex with the guest MCP server
```

Use `just run shoes` instead of `just mcp shoes` to host the shop inside its own
Codex session. Both business peers advertise independently using mDNS. Inspect
the peer named **Kiez Kicks** and its `shoes` plugin, then call `open_peer_web`
for the browser storefront. The peer's base API URL is not the website URL.

Try this guest prompt:

```text
Discover the local peer named Kiez Kicks. Read its shoes catalog and available
EU sizes. Open its website, then reserve one Ringbahn Runner in size 41 for
pickup. Use a fresh request ID and reuse it for retries. This is a simulation.
```

Styles: Ringbahn Runner (EUR 125), Kreuzberg Canvas (EUR 79), Afterhours High-Top
(EUR 149), and Tempelhof Trainer (EUR 99). Every style has EU sizes 36–46, with
one pair in size 41 and two pairs in every other size at startup.

## WIT API

Interface: `berlin:shoes/inventory@0.1.0`.

| Function | Arguments | Result |
| --- | --- | --- |
| `get-catalog` | none | Styles, prices in cents, live stock by size |
| `reserve` | `product-id`, `size`, `request-id` | One pair reserved for pickup |
| `get-reservation` | `id` | Optional reservation |
| `get-reservations` | none | All demo holds, including cancelled ones |
| `cancel` | `id` | Cancels a hold and releases stock |

`reserve` and `cancel` return typed errors. Request IDs must be 8–64 ASCII
letters/digits or `._-`. Identical retries return the same hold, including after
cancellation. Reusing an ID with different inputs fails. Concurrent calls cannot
oversell stock. Cancellations are idempotent. The ledger holds at most 256
reservations and resets, along with stock, on restart/reconnection.

The website uses `GET /api/catalog`, `GET /api/reservations`, and JSON
`POST /api/reserve` or `/api/cancel` with snake_case input keys. Browser and agent
actions call the same guest functions. The shelf and pickup desk refresh every
1.5 seconds while visible. Agent calls use WIT's kebab-case keys.

## Rebuild

The fixture `fixtures/shoe-store.wasm` is ready to run. To rebuild it:

```sh
rustup target add wasm32-unknown-unknown
python3 examples/shoe-store/build.py
cargo test --locked --test shoe_store
```

The guest crate has its own Cargo.lock. WIT remains the only runtime interface
description. The component imports no host filesystem or network capabilities.
