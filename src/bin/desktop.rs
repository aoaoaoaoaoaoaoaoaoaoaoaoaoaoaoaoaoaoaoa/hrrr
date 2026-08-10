#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
#![expect(
    unused_crate_dependencies,
    reason = "package dependencies belong to the shared HRRR library target"
)]

fn main() -> anyhow::Result<()> {
    hrrr::run()
}
