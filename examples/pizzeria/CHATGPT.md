# Connect Hyperlocal to ChatGPT

ChatGPT plugin connections use an HTTP MCP endpoint or Secure MCP Tunnel. Adding
a stdio server to Codex's local settings alone does not establish that plugin
connection.

## Start the local guest HTTP endpoint

```sh
just chatgpt
```

This runs the guest MCP server at `http://127.0.0.1:47832/mcp`. Start the
restaurant separately using `just mcp restaurant` or `just run restaurant`.
The guest runs on the device connected to the restaurant Wi-Fi; ChatGPT sends
tool calls to it, and it performs mDNS discovery and peer calls locally.

To use another loopback port:

```sh
just chatgpt --http 127.0.0.1:47833
```

The endpoint implements MCP Streamable HTTP with JSON responses via the Rust
MCP SDK. Its eleven tools are the same as stdio. HTTP requests share one runtime,
download cache, and set of retained plugin instances and web mounts. No separate
HTTP session ID is needed. Ctrl-C stops the server and closes those mounts.

The endpoint accepts only loopback bindings and validates Host and Origin
headers. It has no built-in account authentication: connect it through Secure
MCP Tunnel, which controls access using your OpenAI organization/workspace.
Do not forward this endpoint to an unauthenticated public URL. A public hosting
deployment would need authentication and explicit proxy/hostname configuration.

## Create a private tunnel

Follow the current [Secure MCP Tunnel documentation](https://developers.openai.com/api/docs/guides/secure-mcp-tunnels).
Create a tunnel in Platform tunnel settings and associate the intended ChatGPT
workspace. Install the official `tunnel-client` from the download link in those
settings. You need Tunnels Read + Manage to create a tunnel, and Read + Use to
run/select one. Account and workspace availability must be checked separately.

With `CONTROL_PLANE_API_KEY` set securely in your local environment (never paste
it into chat or commit it), configure the client:

```sh
tunnel-client init \
  --sample sample_mcp_remote_no_auth \
  --profile hyperlocal \
  --tunnel-id <your-tunnel-id> \
  --mcp-server-url http://127.0.0.1:47832/mcp
tunnel-client doctor --profile hyperlocal --explain
tunnel-client run --profile hyperlocal
```

Keep both the HTTP server and tunnel client running. Use the tunnel client's
readiness/admin UI to verify active polling before registering the plugin.
If the installed client's flags differ, run `tunnel-client help quickstart`.

## Register and install the plugin

At [ChatGPT Plugins](https://chatgpt.com/plugins), select **+ → Add custom MCP
server**. Name it **Hyperlocal**, choose **Tunnel**, and select or paste the
tunnel ID. The private guest endpoint does not implement OAuth, so choose
**No authentication** for the MCP server itself; tunnel access remains governed
by the OpenAI tunnel identity and workspace permissions.

Review the tools and risk notice, create the plugin, and install it for your
account. Do not publish it workspace-wide unless you intend to share access to
the network/device running this guest server. Select it with **@Hyperlocal** in
a conversation and ask:

```text
Use Hyperlocal to discover Forno Pizza on my network. Inspect its restaurant
plugin, read the menu, and show the available pizzas. Do not order yet.
```

After verifying the correct peer, ask to place a simulated order. The local
restaurant back office sees the same order. Remote browser views returned by
the guest are loopback URLs: open them on the device running Hyperlocal, not
inside a cloud browser or on another device. No embedded ChatGPT widget is
provided; the menu remains the component's website.

See [the official connection guide](https://developers.openai.com/plugins/deploy/connect-chatgpt)
for current account permissions, refresh, and installation steps.
