#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    winresource::WindowsResource::new()
        .set_icon("assets/hrrr.ico")
        .compile()?;
    Ok(())
}

#[cfg(not(windows))]
fn main() {}
