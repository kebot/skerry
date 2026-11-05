# Per-Connection local CDP proxy as the Agent endpoint

The app can host a local CDP proxy for each Connection (off by default, bound to `127.0.0.1`). An Agent attaches to the proxy instead of the Manager. The proxy adds the Manager token upstream, so Agents never see it, and lets the app mark the Tabs the Agent touched. Any CDP-speaking Agent works unchanged.

**Costs:** the app proxies CDP traffic and carries extra code to maintain.

## Considered Options

- **Agents connect directly to the Manager's `/cdp`.** Simplest, and the Manager does allow multiple clients (verified from source: each client gets its own upstream connection). But the Agent must hold the Manager token, and the app cannot tell which Tabs the Agent touched.
- **The app hosts its own MCP server.** Would tie the app to one Agent protocol and require reimplementing browser tools; a CDP proxy serves every CDP-speaking Agent.

## Open

The Manager side supports concurrent clients, but Chromium accepting concurrent CDP sessions (UI plus Agent) on the same page is still to be verified in practice (see TASK.md, T1/T16).
