//! Agent-facing automation: an MCP server (`rmcp`) over the engine's command registry.
//!
//! L4, no UI-toolkit dependencies. Headless mode drives an in-process engine session; bridge mode
//! forwards to a running desktop app, so agents see the same state as the user.
