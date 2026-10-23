//! End-to-end tests of the `gymgar-cli` binary.

// A test-only crate: failing loudly is the point. `clippy.toml` relaxes these lints inside `#[test]` functions only,
// not in the helpers of an integration-test crate.
#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

mod mcp;
mod usage;
