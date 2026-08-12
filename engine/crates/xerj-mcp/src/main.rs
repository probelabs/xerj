//! Standalone `xerj-mcp` binary — a thin wrapper over [`xerj_mcp::run`].
//!
//! The same entry point is reachable as `xerj mcp` on the main binary, which
//! is the path a user who ran the published installer has. This binary stays
//! because CI builds and smoke-tests it, and because embedding the MCP proxy
//! without the engine is occasionally useful.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    xerj_mcp::run(&args).await
}
