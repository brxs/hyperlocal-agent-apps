//! Convert a wit-bindgen core module into a standalone component without a WASI adapter.
use anyhow::{Context, Result, ensure};
fn main() -> Result<()> {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    ensure!(
        arguments.len() == 2,
        "usage: package_component INPUT.wasm OUTPUT.wasm"
    );
    let module = std::fs::read(&arguments[0]).context("cannot read core module")?;
    let component = wit_component::ComponentEncoder::default()
        .module(&module)?
        .validate(true)
        .encode()?;
    std::fs::write(&arguments[1], component).context("cannot write component")?;
    Ok(())
}
