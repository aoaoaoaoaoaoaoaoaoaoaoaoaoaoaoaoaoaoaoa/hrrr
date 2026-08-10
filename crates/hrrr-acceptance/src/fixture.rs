use std::{fs::File, path::Path};

use egui_tester::{Error, Result, Testbed};
use fast_mvt::{DEFAULT_EXTENT, MvtFeature, MvtGeometry, MvtLayer, MvtTile, MvtValue};
use geo_types::{line_string, polygon};
use pmtiles::{Compression, PmTilesWriter, TileCoord, TileType};

pub struct FixtureWorld;

impl FixtureWorld {
    pub fn raise(testbed: &Testbed) -> Result<Self> {
        let archive = testbed.private_path("fixtures/basemap.pmtiles")?;
        let _fixtures = testbed.create_private_dir("fixtures")?;
        let _i3 = testbed.write_private(
            "fixtures/i3.config",
            b"font pango:monospace 8\n\
default_border pixel 1\n\
focus_follows_mouse no\n\
bar {\n\
    position top\n\
    tray_output screen\n\
    colors {\n\
        background #1d1f21\n\
        statusline #e0e0e0\n\
    }\n\
}\n",
        )?;
        forge_basemap(&archive)?;
        Ok(Self)
    }
}

fn forge_basemap(path: &Path) -> Result<()> {
    let file = File::create(path).map_err(|source| Error::Io {
        operation: "create fixture basemap",
        path: path.to_owned(),
        source,
    })?;
    let mut writer = PmTilesWriter::new(TileType::Mvt)
        .internal_compression(Compression::None)
        .tile_compression(Compression::None)
        .min_zoom(0)
        .max_zoom(11)
        .bounds(-180.0, -85.0, 180.0, 85.0)
        .center(-98.5, 39.5)
        .center_zoom(4)
        .metadata(
            r#"{"vector_layers":[{"id":"earth"},{"id":"landcover"},{"id":"water"},{"id":"roads"}]}"#,
        )
        .create(file)
        .map_err(|error| verdict(format!("raise fixture PMTiles writer: {error}")))?;
    let tile = vector_tile()?;
    for zoom in 0_u8..=6 {
        let side = 1_u32 << zoom;
        for y in 0..side {
            for x in 0..side {
                let coordinate = TileCoord::new(zoom, x, y)
                    .map_err(|error| verdict(format!("forge fixture tile: {error}")))?;
                writer
                    .add_tile(coordinate, &tile)
                    .map_err(|error| verdict(format!("write fixture vector tile: {error}")))?;
            }
        }
    }
    writer
        .finalize()
        .map_err(|error| verdict(format!("seal fixture PMTiles archive: {error}")))
}

fn vector_tile() -> Result<Vec<u8>> {
    let polygon = |west, north, east, south| {
        MvtGeometry::Polygon(polygon![
            (x: west, y: north),
            (x: east, y: north),
            (x: east, y: south),
            (x: west, y: south),
            (x: west, y: north),
        ])
    };
    let feature = |id, geometry, properties| MvtFeature {
        id: Some(id),
        geometry,
        properties,
    };
    MvtTile {
        layers: vec![
            MvtLayer {
                name: "earth".to_owned(),
                extent: DEFAULT_EXTENT,
                features: vec![feature(1, polygon(0, 0, 4_096, 4_096), Vec::new())],
            },
            MvtLayer {
                name: "landcover".to_owned(),
                extent: DEFAULT_EXTENT,
                features: vec![feature(
                    2,
                    polygon(2_080, 180, 3_900, 3_900),
                    vec![("kind".to_owned(), MvtValue::String("forest".to_owned()))],
                )],
            },
            MvtLayer {
                name: "water".to_owned(),
                extent: DEFAULT_EXTENT,
                features: vec![feature(3, polygon(680, 520, 1_620, 3_700), Vec::new())],
            },
            MvtLayer {
                name: "roads".to_owned(),
                extent: DEFAULT_EXTENT,
                features: vec![feature(
                    4,
                    MvtGeometry::LineString(line_string![
                        (x: 120, y: 3_600),
                        (x: 1_850, y: 2_080),
                        (x: 3_980, y: 620),
                    ]),
                    vec![
                        ("kind".to_owned(), MvtValue::String("major_road".to_owned())),
                        (
                            "kind_detail".to_owned(),
                            MvtValue::String("secondary".to_owned()),
                        ),
                        ("min_zoom".to_owned(), MvtValue::Double(0.0)),
                    ],
                )],
            },
        ],
    }
    .encode()
    .map_err(|error| verdict(format!("encode fixture vector tile: {error}")))
}

fn verdict(detail: impl Into<String>) -> Error {
    Error::Verdict {
        detail: detail.into(),
    }
}
