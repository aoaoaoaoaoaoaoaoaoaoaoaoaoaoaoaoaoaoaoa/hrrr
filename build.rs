use std::{env, error::Error, path::PathBuf};

#[path = "build/wind_barb.rs"]
mod wind_barb;

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=build/wind_barb.rs");
    wind_barb::bake(&PathBuf::from(env::var("OUT_DIR")?).join("wind_barb.rs"))?;
    platform_resources()?;
    Ok(())
}

#[cfg(windows)]
fn platform_resources() -> Result<(), Box<dyn Error>> {
    winresource::WindowsResource::new()
        .set_icon("assets/hrrr.ico")
        .compile()?;
    Ok(())
}

#[cfg(not(windows))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "shares the fallible Windows resource-build contract"
)]
fn platform_resources() -> Result<(), Box<dyn Error>> {
    Ok(())
}
