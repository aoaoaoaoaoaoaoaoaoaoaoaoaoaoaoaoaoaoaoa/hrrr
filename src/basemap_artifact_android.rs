//! Android's streamed Protomaps source contract.

use crate::application_paths::ApplicationPaths;
use anyhow::Result;

pub const LOCAL_MAX_ZOOM: u8 = 11;
pub const MAX_ZOOM: u8 = 12;
pub const BOUNDS: [f64; 4] = [-135.0, 21.0, -60.0, 54.0];

const GENERATION: &str = "20260823";
const MAP_ORIGIN: &str = "https://build.protomaps.com";

#[derive(Clone, Debug)]
pub struct DetailSource {
    pub url: String,
    pub generation: String,
}

#[allow(
    clippy::unnecessary_wraps,
    reason = "the Android projection preserves the fallible desktop basemap-source seam"
)]
pub fn detail_source(_paths: &ApplicationPaths) -> Result<Option<DetailSource>> {
    Ok(Some(DetailSource {
        url: format!("{MAP_ORIGIN}/{GENERATION}.pmtiles"),
        generation: GENERATION.to_owned(),
    }))
}
