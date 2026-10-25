//! Fence clients that cannot preserve document conversation continuation semantics.
use anyhow::Result;

pub fn run() -> Result<()> {
    tracing::info!(
        "v052: fenced document conversation launch semantics; Session rows are unchanged"
    );
    Ok(())
}
