use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

/// OpenAPI document the web client is generated from, relative to the workspace root.
pub(crate) const OPENAPI_PATH: &str = "web/src/shared/api/openapi.json";

pub(crate) fn openapi(root: &Path) -> Result<()> {
    let mut json =
        nullradix_api::openapi_json().context("failed to render the OpenAPI document")?;
    json.push('\n');
    let path = root.join(OPENAPI_PATH);
    fs::write(&path, json).with_context(|| format!("failed to write {}", path.display()))?;
    println!("wrote {OPENAPI_PATH}");
    Ok(())
}

pub(crate) fn codegen(root: &Path) -> Result<()> {
    openapi(root)?;
    let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
    let status = Command::new(npm)
        .args(["run", "generate", "-w", "web"])
        .current_dir(root)
        .status()
        .with_context(|| format!("failed to run {npm}"))?;
    if !status.success() {
        bail!("npm run generate failed with {status}");
    }
    Ok(())
}
