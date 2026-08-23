use crate::{
    application_paths::ApplicationPaths,
    basemap_artifact::{
        BOUNDS, DetailSource, LOCAL_MAX_ZOOM, MAX_ZOOM as MAX_SOURCE_ZOOM, detail_source,
    },
    cache::CacheStore,
    map,
    model::Viewport,
};
use anyhow::{Context as _, Result};
use bytemuck::{Pod, Zeroable};
use bytes::Bytes;
use crossbeam_channel::{Receiver, Sender, bounded};
use egui::Context;
use eternalist_apps::NativeWake;
use fast_mvt::{MvtFeatureRef, MvtGeometry, MvtReaderRef, MvtValueRef};
use geo_types::{Coord, LineString, Point, Polygon};
use lyon_tessellation::{
    BuffersBuilder, FillOptions, FillRule, FillTessellator, FillVertex, VertexBuffers, math::point,
    path::Path,
};
use pmtiles::{AsyncPmTilesReader, HashMapCache, HttpBackend, MmapBackend, TileCoord};
use std::{
    path::{Path as FsPath, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, SystemTime},
};

pub const PAPER_SRGB: [u8; 3] = [229; 3];
pub const APPARITION_SPAN: f32 = 1.35;
const RETAINED_DEPTH: u8 = 4;
const DETAIL_CONNECT_LIMIT: Duration = Duration::from_secs(8);
const DETAIL_TRANSFER_LIMIT: Duration = Duration::from_secs(30);

pub fn apparition(view_zoom: f32, onset_zoom: f32) -> f32 {
    let phase = ((view_zoom - onset_zoom) / APPARITION_SPAN).clamp(0.0, 1.0);
    phase * phase * (3.0 - 2.0 * phase)
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TileKey {
    pub zoom: u8,
    pub x: u32,
    pub y: u32,
}

impl TileKey {
    pub const fn is_detail(self) -> bool {
        self.zoom > LOCAL_MAX_ZOOM
    }

    fn coordinate(self) -> Result<TileCoord> {
        TileCoord::new(self.zoom, self.x, self.y).context("invalid PMTiles coordinate")
    }
}

#[derive(Debug)]
pub struct Cover {
    pub strata: Vec<Stratum>,
}

impl Cover {
    pub fn finest_ready(&self, mut resident: impl FnMut(TileKey) -> bool) -> Option<&Stratum> {
        self.strata.iter().rev().find(|stratum| {
            stratum.intent.presents() && stratum.keys.iter().all(|key| resident(*key))
        })
    }
}

#[derive(Debug)]
pub struct Stratum {
    pub intent: Intent,
    pub keys: Vec<TileKey>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Intent {
    Retained,
    Required,
    Prefetch,
}

impl Intent {
    pub const fn demands(self) -> bool {
        matches!(self, Self::Required | Self::Prefetch)
    }

    pub const fn presents(self) -> bool {
        !matches!(self, Self::Prefetch)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct FillPoint {
    pub local: [f32; 2],
    pub material: u32,
    pub onset_zoom: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct StrokePoint {
    pub local: [f32; 2],
    pub extrusion: [f32; 2],
    pub srgb: [u8; 4],
    pub radius_points: f32,
    /// Magnitude is `onset_zoom + 1`; sign selects the extrusion bank.
    pub onset_side: f32,
}

#[derive(Clone, Debug)]
pub struct Label {
    pub world: [f64; 2],
    pub text: Arc<str>,
    pub rank: u16,
    pub size: f32,
    pub onset_zoom: f32,
}

#[derive(Clone, Debug, Default)]
pub struct Mesh<V> {
    pub vertices: Arc<[V]>,
    pub indices: Arc<[u32]>,
}

#[derive(Clone, Debug)]
pub struct VectorTile {
    pub key: TileKey,
    pub fills: Mesh<FillPoint>,
    pub strokes: Mesh<StrokePoint>,
    pub labels: Arc<[Label]>,
}

impl VectorTile {
    pub fn resident_bytes(&self) -> usize {
        self.fills
            .vertices
            .len()
            .saturating_mul(size_of::<FillPoint>())
            .saturating_add(self.fills.indices.len().saturating_mul(size_of::<u32>()))
            .saturating_add(
                self.strokes
                    .vertices
                    .len()
                    .saturating_mul(size_of::<StrokePoint>()),
            )
            .saturating_add(self.strokes.indices.len().saturating_mul(size_of::<u32>()))
            .saturating_add(self.labels.len().saturating_mul(size_of::<Label>()))
            .saturating_add(
                self.labels
                    .iter()
                    .map(|label| label.text.len())
                    .sum::<usize>(),
            )
    }
}

#[derive(Debug)]
pub enum Event {
    Ready,
    Loaded(Arc<VectorTile>),
    Missing(TileKey),
    Fault {
        key: Option<TileKey>,
        message: String,
    },
}

pub struct Basemap {
    commands: Sender<TileKey>,
    pub events: Receiver<Event>,
    _thread: thread::JoinHandle<()>,
}

impl Basemap {
    pub fn spawn(ctx: Context, paths: &ApplicationPaths) -> Result<Self> {
        let archive = paths.basemap_path()?;
        if !archive.is_file() {
            anyhow::bail!(
                "no basemap archive at {}; run `hrrr basemap install`",
                archive.display()
            );
        }
        let workers = thread::available_parallelism()
            .map_or(4, std::num::NonZeroUsize::get)
            .clamp(2, 8);
        let detail = detail_source(paths)?.map(|source| Detail::new(source, paths.basemap_cache()));
        Self::spawn_with_workers(ctx, archive, detail, workers)
    }

    fn spawn_with_workers(
        ctx: Context,
        archive: PathBuf,
        detail: Option<Detail>,
        workers: usize,
    ) -> Result<Self> {
        if let Some(directory) = archive.parent() {
            purge_partials(directory)?;
        }
        let (commands, command_rx) = bounded(256);
        let (event_tx, events) = bounded(256);
        let wake = NativeWake::from_context(&ctx);
        let thread = thread::Builder::new()
            .name("vector-armory".to_owned())
            .spawn(move || armory(wake, archive, detail, command_rx, event_tx, workers))
            .context("spawn vector basemap armory")?;
        Ok(Self {
            commands,
            events,
            _thread: thread,
        })
    }

    pub fn request(&self, key: TileKey) -> bool {
        self.commands.try_send(key).is_ok()
    }
}

pub fn cover(view: Viewport, rect: egui::Rect) -> Cover {
    let zoom = view.zoom.floor().clamp(0.0, f64::from(MAX_SOURCE_ZOOM)) as u8;
    let ceiling = zoom
        .saturating_add(1)
        .min(MAX_SOURCE_ZOOM)
        .min(LOCAL_MAX_ZOOM.max(zoom));
    let strata = (zoom.saturating_sub(RETAINED_DEPTH)..=ceiling)
        .map(|level| Stratum {
            intent: match level.cmp(&zoom) {
                std::cmp::Ordering::Less => Intent::Retained,
                std::cmp::Ordering::Equal => Intent::Required,
                std::cmp::Ordering::Greater => Intent::Prefetch,
            },
            keys: keys_at(view, rect, level),
        })
        .collect();
    Cover { strata }
}

fn keys_at(view: Viewport, rect: egui::Rect, zoom: u8) -> Vec<TileKey> {
    let divisions = 1_u32 << zoom;
    let bounds = map::world_bounds(view, rect);
    let scale = f64::from(divisions);
    let left = (bounds[0] * scale).floor() as i64;
    let right = (bounds[2] * scale).floor() as i64;
    let top = (bounds[1] * scale).floor().max(0.0) as i64;
    let bottom = (bounds[3] * scale)
        .floor()
        .min(f64::from(divisions.saturating_sub(1))) as i64;
    let mut keys = Vec::new();
    for raw_y in top..=bottom {
        for raw_x in left..=right {
            keys.push(TileKey {
                zoom,
                x: raw_x.rem_euclid(i64::from(divisions)) as u32,
                y: raw_y as u32,
            });
        }
    }
    keys.sort_unstable();
    keys.dedup();
    keys.sort_unstable_by(|left, right| {
        tile_distance(*left, view.center_mercator)
            .total_cmp(&tile_distance(*right, view.center_mercator))
    });
    keys
}

fn tile_distance(key: TileKey, center: [f64; 2]) -> f64 {
    let scale = f64::from(1_u32 << key.zoom);
    let x = (f64::from(key.x) + 0.5) / scale;
    let y = (f64::from(key.y) + 0.5) / scale;
    (x - center[0]).mul_add(x - center[0], (y - center[1]).powi(2))
}

type Archive = AsyncPmTilesReader<MmapBackend, HashMapCache>;
type DetailArchive = AsyncPmTilesReader<HttpBackend, HashMapCache>;

struct Detail {
    source: DetailSource,
    cache: CacheStore,
    archive: Mutex<Option<Arc<DetailArchive>>>,
}

impl Detail {
    const fn new(source: DetailSource, cache: CacheStore) -> Self {
        Self {
            source,
            cache,
            archive: Mutex::new(None),
        }
    }

    fn fetch(&self, runtime: &tokio::runtime::Runtime, key: TileKey) -> Result<Option<Bytes>> {
        if key.zoom <= LOCAL_MAX_ZOOM || !within_detail_bounds(key) {
            return Ok(None);
        }
        let blade = PathBuf::from(&self.source.generation)
            .join(key.zoom.to_string())
            .join(key.x.to_string())
            .join(format!("{}.mvt", key.y));
        if let Some(bytes) = self.cache.recall(&blade, valid_mvt)? {
            return Ok(Some(bytes.into()));
        }
        let archive = self.open(runtime)?;
        let bytes = runtime
            .block_on(archive.get_tile_decompressed(key.coordinate()?))
            .map_err(anyhow::Error::new)
            .context("fetch detailed basemap tile")?;
        let Some(bytes) = bytes else {
            return Ok(None);
        };
        if !valid_mvt(&bytes) {
            anyhow::bail!("remote basemap returned an invalid vector tile");
        }
        self.cache.write(&blade, &bytes)?;
        Ok(Some(bytes))
    }

    fn open(&self, runtime: &tokio::runtime::Runtime) -> Result<Arc<DetailArchive>> {
        let mut slot = self
            .archive
            .lock()
            .map_err(|_| anyhow::anyhow!("detailed basemap archive lock was poisoned"))?;
        if let Some(archive) = slot.as_ref() {
            return Ok(Arc::clone(archive));
        }
        let client = pmtiles::reqwest::Client::builder()
            .user_agent(concat!("hrrr/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(DETAIL_CONNECT_LIMIT)
            .timeout(DETAIL_TRANSFER_LIMIT)
            .build()
            .context("build basemap detail client")?;
        let archive = runtime
            .block_on(DetailArchive::new_with_cached_url(
                HashMapCache::default(),
                client,
                &self.source.url,
            ))
            .map_err(anyhow::Error::new)
            .context("open remote basemap detail")?;
        let archive = Arc::new(archive);
        *slot = Some(Arc::clone(&archive));
        Ok(archive)
    }
}

fn valid_mvt(bytes: &[u8]) -> bool {
    MvtReaderRef::new(bytes).is_ok()
}

fn within_detail_bounds(key: TileKey) -> bool {
    let scale = f64::from(1_u32 << key.zoom);
    let west = f64::from(key.x) / scale * 360.0 - 180.0;
    let east = f64::from(key.x + 1) / scale * 360.0 - 180.0;
    let north = map::lon_lat_at([0.0, f64::from(key.y) / scale])[1];
    let south = map::lon_lat_at([0.0, f64::from(key.y + 1) / scale])[1];
    east >= BOUNDS[0] && west <= BOUNDS[2] && north >= BOUNDS[1] && south <= BOUNDS[3]
}

fn armory(
    wake: NativeWake,
    archive: PathBuf,
    detail: Option<Detail>,
    commands: Receiver<TileKey>,
    events: Sender<Event>,
    worker_count: usize,
) {
    let runtime = match runtime() {
        Ok(runtime) => runtime,
        Err(err) => {
            send_fault(&wake, &events, None, &err);
            return;
        }
    };
    let reader = match runtime.block_on(Archive::new_with_cached_path(
        HashMapCache::default(),
        &archive,
    )) {
        Ok(reader) => Arc::new(reader),
        Err(err) => {
            send_fault(
                &wake,
                &events,
                None,
                &anyhow::Error::new(err).context(format!("open {}", archive.display())),
            );
            return;
        }
    };
    if events.send(Event::Ready).is_err() {
        return;
    }
    let _woken = wake.request_foreground_repaint();
    let detail = detail.map(Arc::new);
    let mut workers = Vec::with_capacity(worker_count);
    for slot in 0..worker_count {
        let worker_wake = wake.clone();
        let reader = reader.clone();
        let detail = detail.clone();
        let commands = commands.clone();
        let worker_events = events.clone();
        let worker = thread::Builder::new()
            .name(format!("vector-quarry-{slot}"))
            .spawn(move || quarry(worker_wake, reader, detail, commands, worker_events));
        match worker {
            Ok(worker) => workers.push(worker),
            Err(err) => {
                send_fault(
                    &wake,
                    &events,
                    None,
                    &anyhow::Error::new(err).context("spawn vector quarry"),
                );
                break;
            }
        }
    }
    drop(commands);
    drop(events);
    for worker in workers {
        let _joined = worker.join();
    }
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build basemap runtime")
}

fn quarry(
    wake: NativeWake,
    archive: Arc<Archive>,
    detail: Option<Arc<Detail>>,
    commands: Receiver<TileKey>,
    events: Sender<Event>,
) {
    let runtime = match runtime() {
        Ok(runtime) => runtime,
        Err(err) => {
            send_fault(&wake, &events, None, &err);
            return;
        }
    };
    while let Ok(key) = commands.recv() {
        let bytes = match load_tile(&runtime, &archive, detail.as_deref(), key) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => {
                if events.send(Event::Missing(key)).is_err() {
                    break;
                }
                let _woken = wake.request_foreground_repaint();
                continue;
            }
            Err(err) => {
                send_fault(&wake, &events, Some(key), &err);
                continue;
            }
        };
        let event = match decode_tile(key, &bytes) {
            Ok(tile) => Event::Loaded(Arc::new(tile)),
            Err(err) => Event::Fault {
                key: Some(key),
                message: format!("decode vector tile {key:?}: {err:#}"),
            },
        };
        if events.send(event).is_err() {
            break;
        }
        let _woken = wake.request_foreground_repaint();
    }
}

fn load_tile(
    runtime: &tokio::runtime::Runtime,
    archive: &Archive,
    detail: Option<&Detail>,
    key: TileKey,
) -> Result<Option<Bytes>> {
    let local = runtime
        .block_on(archive.get_tile_decompressed(key.coordinate()?))
        .map_err(anyhow::Error::new)?;
    match local {
        Some(bytes) => Ok(Some(bytes)),
        None => detail.map_or(Ok(None), |detail| detail.fetch(runtime, key)),
    }
}

fn send_fault(
    wake: &NativeWake,
    events: &Sender<Event>,
    key: Option<TileKey>,
    err: &anyhow::Error,
) {
    if events
        .try_send(Event::Fault {
            key,
            message: format!("{err:#}"),
        })
        .is_ok()
    {
        let _woken = wake.request_foreground_repaint();
    }
}

fn decode_tile(key: TileKey, bytes: &[u8]) -> Result<VectorTile> {
    let reader = MvtReaderRef::new(bytes).context("parse MVT")?;
    let mut forge = Forge::new(key);
    for layer in reader.layers() {
        match layer.name() {
            "earth" => forge.fill_layer(layer, FillKind::Earth)?,
            "landcover" => forge.fill_layer(layer, FillKind::Landcover)?,
            "landuse" => forge.fill_layer(layer, FillKind::Landuse)?,
            "water" => forge.water_layer(layer)?,
            "boundaries" => forge.stroke_layer(layer, StrokeKind::Boundary)?,
            "roads" => forge.stroke_layer(layer, StrokeKind::Road)?,
            "places" => forge.label_layer(layer)?,
            _ => {}
        }
    }
    Ok(forge.finish())
}

#[derive(Clone, Copy)]
enum FillKind {
    Earth,
    Landcover,
    Landuse,
}

impl FillKind {
    fn material(self, kind: Option<&str>) -> Option<FillMaterial> {
        match self {
            Self::Earth => Some(FillMaterial::Earth),
            Self::Landcover => match kind {
                Some("forest" | "wood") => Some(FillMaterial::Forest),
                Some("grass" | "grassland" | "scrub") => Some(FillMaterial::Grassland),
                _ => None,
            },
            Self::Landuse => match kind {
                Some("forest" | "wood" | "nature_reserve") => Some(FillMaterial::Forest),
                Some("park" | "garden" | "grass" | "grassland" | "meadow") => {
                    Some(FillMaterial::Grassland)
                }
                Some("wetland") => Some(FillMaterial::Wetland),
                Some("farmland") => Some(FillMaterial::Farmland),
                Some("beach" | "sand") => Some(FillMaterial::Sand),
                Some("industrial" | "commercial" | "retail" | "railway") => {
                    Some(FillMaterial::Industry)
                }
                Some("school" | "college" | "university" | "hospital") => Some(FillMaterial::Civic),
                Some("residential") => Some(FillMaterial::Residential),
                _ => None,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
enum FillMaterial {
    Earth,
    Forest,
    Grassland,
    Wetland,
    Farmland,
    Sand,
    Industry,
    Civic,
    Residential,
    Water,
}

#[derive(Clone, Copy)]
enum StrokeKind {
    Boundary,
    Road,
}

#[derive(Clone, Copy)]
struct StrokeStyle {
    color: [u8; 4],
    radius_points: f32,
    onset_zoom: f32,
}

const WATER_STROKE: StrokeStyle = StrokeStyle {
    color: [91, 91, 91, 112],
    radius_points: 0.36,
    onset_zoom: 0.0,
};

struct Forge {
    key: TileKey,
    fills: VertexBuffers<FillPoint, u32>,
    strokes: VertexBuffers<StrokePoint, u32>,
    labels: Vec<Label>,
    tessellator: FillTessellator,
}

impl Forge {
    fn new(key: TileKey) -> Self {
        Self {
            key,
            fills: VertexBuffers::new(),
            strokes: VertexBuffers::new(),
            labels: Vec::new(),
            tessellator: FillTessellator::new(),
        }
    }

    fn fill_layer(&mut self, layer: fast_mvt::MvtLayerRef<'_>, fill: FillKind) -> Result<()> {
        let extent = layer.extent();
        for feature in layer.features() {
            let tags = FeatureTags::read(feature)?;
            let Some(material) = fill.material(tags.kind) else {
                continue;
            };
            self.fill_geometry(
                &feature.geometry()?,
                extent,
                material,
                tags.min_zoom.unwrap_or(0.0) as f32,
            )?;
        }
        Ok(())
    }

    fn water_layer(&mut self, layer: fast_mvt::MvtLayerRef<'_>) -> Result<()> {
        let extent = layer.extent();
        for feature in layer.features() {
            let tags = FeatureTags::read(feature)?;
            let onset_zoom = tags.min_zoom.unwrap_or(0.0) as f32;
            let geometry = feature.geometry()?;
            match geometry {
                MvtGeometry::Polygon(_) | MvtGeometry::MultiPolygon(_) => {
                    self.fill_geometry(&geometry, extent, FillMaterial::Water, onset_zoom)?;
                }
                MvtGeometry::LineString(_) | MvtGeometry::MultiLineString(_) => {
                    self.stroke_geometry(
                        &geometry,
                        extent,
                        StrokeStyle {
                            onset_zoom,
                            ..WATER_STROKE
                        },
                    );
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn stroke_layer(&mut self, layer: fast_mvt::MvtLayerRef<'_>, stroke: StrokeKind) -> Result<()> {
        let extent = layer.extent();
        for feature in layer.features() {
            let style = match stroke {
                StrokeKind::Boundary => Some(boundary_style(
                    integer_property(feature, "kind_detail")?,
                    numeric_property(feature, "min_zoom")?,
                )),
                StrokeKind::Road => {
                    let tags = FeatureTags::read(feature)?;
                    road_style(tags.kind, tags.detail, tags.min_zoom, self.key.zoom)
                }
            };
            let Some(style) = style else {
                continue;
            };
            self.stroke_geometry(&feature.geometry()?, extent, style);
        }
        Ok(())
    }

    fn label_layer(&mut self, layer: fast_mvt::MvtLayerRef<'_>) -> Result<()> {
        let extent = layer.extent();
        for feature in layer.features() {
            let tags = FeatureTags::read(feature)?;
            let Some(name) = tags.name else { continue };
            let Some(style) =
                label_style(tags.kind, tags.detail, tags.population_rank, tags.min_zoom)
            else {
                continue;
            };
            let geometry = feature.geometry()?;
            match geometry {
                MvtGeometry::Point(point) => self.push_label(point, extent, name, style),
                MvtGeometry::MultiPoint(points) => {
                    for point in points {
                        self.push_label(point, extent, name, style);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn push_label(&mut self, point: Point<i32>, extent: u32, text: &str, style: LabelStyle) {
        self.labels.push(Label {
            world: world64(self.key, extent, point.0),
            text: Arc::from(text),
            rank: style.rank,
            size: style.size,
            onset_zoom: style.onset_zoom,
        });
    }

    fn fill_geometry(
        &mut self,
        geometry: &MvtGeometry,
        extent: u32,
        material: FillMaterial,
        onset_zoom: f32,
    ) -> Result<()> {
        match geometry {
            MvtGeometry::Polygon(polygon) => {
                self.fill_polygon(polygon, extent, material, onset_zoom)?;
            }
            MvtGeometry::MultiPolygon(polygons) => {
                for polygon in polygons {
                    self.fill_polygon(polygon, extent, material, onset_zoom)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn fill_polygon(
        &mut self,
        polygon: &Polygon<i32>,
        extent: u32,
        material: FillMaterial,
        onset_zoom: f32,
    ) -> Result<()> {
        let mut path = Path::builder();
        push_ring(&mut path, extent, polygon.exterior());
        for ring in polygon.interiors() {
            push_ring(&mut path, extent, ring);
        }
        let path = path.build();
        self.tessellator
            .tessellate_path(
                &path,
                &FillOptions::default().with_fill_rule(FillRule::EvenOdd),
                &mut BuffersBuilder::new(&mut self.fills, |vertex: FillVertex<'_>| FillPoint {
                    local: vertex.position().to_array(),
                    material: material as u32,
                    onset_zoom,
                }),
            )
            .context("tessellate vector polygon")?;
        Ok(())
    }

    fn stroke_geometry(&mut self, geometry: &MvtGeometry, extent: u32, style: StrokeStyle) {
        match geometry {
            MvtGeometry::LineString(line) => self.stroke_line(line, extent, style),
            MvtGeometry::MultiLineString(lines) => {
                for line in lines {
                    self.stroke_line(line, extent, style);
                }
            }
            _ => {}
        }
    }

    fn stroke_line(&mut self, line: &LineString<i32>, extent: u32, style: StrokeStyle) {
        let mut points = Vec::with_capacity(line.0.len());
        for &coordinate in &line.0 {
            let point = local32(extent, coordinate);
            if points.last().is_none_or(|prior| *prior != point) {
                points.push(point);
            }
        }
        if points.len() < 2 {
            return;
        }
        let Ok(base) = u32::try_from(self.strokes.vertices.len()) else {
            return;
        };
        for slot in 0..points.len() {
            let extrusion = join_normal(&points, slot);
            self.strokes.vertices.extend([
                StrokePoint {
                    local: points[slot],
                    extrusion: [-extrusion[0], -extrusion[1]],
                    srgb: style.color,
                    radius_points: style.radius_points,
                    onset_side: -(style.onset_zoom.max(0.0) + 1.0),
                },
                StrokePoint {
                    local: points[slot],
                    extrusion,
                    srgb: style.color,
                    radius_points: style.radius_points,
                    onset_side: style.onset_zoom.max(0.0) + 1.0,
                },
            ]);
        }
        for slot in 0..points.len() - 1 {
            let Some(offset) = u32::try_from(slot)
                .ok()
                .and_then(|slot| slot.checked_mul(2))
            else {
                return;
            };
            let a = base + offset;
            self.strokes
                .indices
                .extend([a, a + 1, a + 2, a + 1, a + 3, a + 2]);
        }
    }

    fn finish(mut self) -> VectorTile {
        self.labels.sort_unstable_by_key(|label| label.rank);
        VectorTile {
            key: self.key,
            fills: Mesh {
                vertices: self.fills.vertices.into(),
                indices: self.fills.indices.into(),
            },
            strokes: Mesh {
                vertices: self.strokes.vertices.into(),
                indices: self.strokes.indices.into(),
            },
            labels: self.labels.into(),
        }
    }
}

fn boundary_style(detail: Option<i64>, min_zoom: Option<f64>) -> StrokeStyle {
    let (color, radius, fallback_onset) = match detail {
        Some(2 | 3) => ([68, 68, 68, 170], 0.58, 0.0),
        Some(4 | 5) => ([82, 82, 82, 128], 0.38, 3.0),
        Some(6 | 7) => ([96, 96, 96, 88], 0.24, 6.0),
        _ => ([104, 104, 104, 58], 0.18, 8.0),
    };
    StrokeStyle {
        color,
        radius_points: radius,
        onset_zoom: min_zoom.unwrap_or(fallback_onset) as f32,
    }
}

fn road_style(
    kind: Option<&str>,
    detail: Option<&str>,
    min_zoom: Option<f64>,
    source_zoom: u8,
) -> Option<StrokeStyle> {
    let (color, radius, fallback_onset) = match (kind, detail) {
        (Some("highway"), Some("motorway")) => ([70, 70, 70, 158], 0.52, 4.0),
        (Some("highway"), Some("motorway_link")) => ([74, 74, 74, 125], 0.34, 7.0),
        (Some("major_road"), Some("trunk" | "trunk_link")) => ([73, 73, 73, 140], 0.44, 5.0),
        (Some("major_road"), Some("primary" | "primary_link")) => ([76, 76, 76, 124], 0.37, 7.0),
        (Some("major_road"), Some("secondary" | "secondary_link")) => {
            ([82, 82, 82, 102], 0.29, 8.0)
        }
        (Some("major_road"), Some("tertiary" | "tertiary_link")) => ([88, 88, 88, 82], 0.22, 9.0),
        (Some("minor_road"), Some("residential" | "unclassified" | "road")) => {
            ([96, 96, 96, 58], 0.15, 10.5)
        }
        (Some("minor_road"), Some("service" | "alley" | "parking_aisle" | "driveway")) => {
            ([104, 104, 104, 42], 0.11, 11.5)
        }
        (Some("path"), Some("pedestrian" | "cycleway" | "track" | "path" | "footway")) => {
            ([92, 92, 92, 34], 0.09, 11.5)
        }
        (Some("rail"), _) | (_, Some("rail" | "light_rail" | "tram" | "subway")) => {
            ([48, 48, 48, 62], 0.13, 9.0)
        }
        (Some("ferry" | "ferryway"), _) => ([96, 96, 96, 48], 0.11, 8.0),
        (Some("aeroway" | "aerialway"), _) => return None,
        (Some("highway"), _) => ([70, 70, 70, 145], 0.46, 5.0),
        (Some("major_road"), _) => ([82, 82, 82, 102], 0.31, 8.0),
        (Some("minor_road"), _) => ([98, 98, 98, 54], 0.14, 10.5),
        (Some("path"), _) => ([94, 94, 94, 30], 0.08, 11.5),
        _ => return None,
    };
    let onset_zoom = min_zoom.unwrap_or(fallback_onset) as f32;
    (f32::from(source_zoom) + 1.0 >= onset_zoom).then_some(StrokeStyle {
        color,
        radius_points: radius,
        onset_zoom,
    })
}

#[derive(Clone, Copy)]
struct LabelStyle {
    rank: u16,
    size: f32,
    onset_zoom: f32,
}

fn label_style(
    kind: Option<&str>,
    detail: Option<&str>,
    population_rank: Option<f64>,
    min_zoom: Option<f64>,
) -> Option<LabelStyle> {
    let population = population_rank.unwrap_or(0.0).clamp(0.0, 18.0);
    let scarcity = (18.0 - population).round() as u16;
    let (base, size, fallback_onset) = match (kind, detail) {
        (Some("country"), _) => (0, 15.0, 1.0),
        (Some("region"), Some("state" | "province")) => (40, 13.0, 3.0),
        (Some("region"), _) => (50, 12.5, 4.0),
        (Some("locality"), Some("city")) => (
            100,
            10.4 + population * 0.22,
            (11.5 - population * 0.48).clamp(3.0, 10.0),
        ),
        (Some("locality"), Some("town")) => (
            220,
            9.6 + population * 0.16,
            (13.0 - population * 0.34).clamp(7.0, 11.5),
        ),
        (Some("locality"), Some("village")) => (
            340,
            9.2 + population * 0.11,
            (13.5 - population * 0.25).clamp(9.0, 12.0),
        ),
        (Some("locality"), Some("hamlet" | "locality")) => (
            460,
            8.7 + population * 0.07,
            (14.0 - population * 0.20).clamp(10.5, 12.0),
        ),
        (Some("macrohood"), _) => (560, 9.5, 10.0),
        (Some("neighbourhood"), Some("suburb")) => (620, 9.2, 10.5),
        (Some("neighbourhood"), _) => (700, 8.8, 11.5),
        _ => return None,
    };
    Some(LabelStyle {
        rank: base + scarcity.saturating_mul(4),
        size: size as f32,
        onset_zoom: min_zoom.unwrap_or(fallback_onset) as f32,
    })
}

#[derive(Default)]
struct FeatureTags<'a> {
    kind: Option<&'a str>,
    detail: Option<&'a str>,
    name: Option<&'a str>,
    population_rank: Option<f64>,
    min_zoom: Option<f64>,
}

impl<'a> FeatureTags<'a> {
    fn read(feature: MvtFeatureRef<'a>) -> Result<Self> {
        let mut tags = Self::default();
        for property in feature.properties() {
            let (key, value) = property?;
            match (key, value) {
                ("kind", MvtValueRef::String(value)) => tags.kind = Some(value),
                ("kind_detail", MvtValueRef::String(value)) => tags.detail = Some(value),
                ("name:en", MvtValueRef::String(value)) => tags.name = Some(value),
                ("name", MvtValueRef::String(value)) if tags.name.is_none() => {
                    tags.name = Some(value);
                }
                ("population_rank", value) => tags.population_rank = numeric(value),
                ("min_zoom", value) => tags.min_zoom = numeric(value),
                _ => {}
            }
        }
        Ok(tags)
    }
}

fn integer_property(feature: MvtFeatureRef<'_>, needle: &str) -> Result<Option<i64>> {
    for property in feature.properties() {
        let (key, value) = property?;
        if key == needle {
            return Ok(integer(value));
        }
    }
    Ok(None)
}

fn numeric_property(feature: MvtFeatureRef<'_>, needle: &str) -> Result<Option<f64>> {
    for property in feature.properties() {
        let (key, value) = property?;
        if key == needle {
            return Ok(numeric(value));
        }
    }
    Ok(None)
}

fn numeric(value: MvtValueRef<'_>) -> Option<f64> {
    match value {
        MvtValueRef::Float(value) => Some(f64::from(value)),
        MvtValueRef::Double(value) => Some(value),
        MvtValueRef::Int(value) | MvtValueRef::SInt(value) => Some(value as f64),
        MvtValueRef::UInt(value) => Some(value as f64),
        _ => None,
    }
}

fn integer(value: MvtValueRef<'_>) -> Option<i64> {
    match value {
        MvtValueRef::Int(value) | MvtValueRef::SInt(value) => Some(value),
        MvtValueRef::UInt(value) => i64::try_from(value).ok(),
        _ => None,
    }
}

fn push_ring(
    path: &mut lyon_tessellation::path::path::Builder,
    extent: u32,
    ring: &LineString<i32>,
) {
    let Some((first, rest)) = ring.0.split_first() else {
        return;
    };
    let first = local32(extent, *first);
    let _first = path.begin(point(first[0], first[1]));
    for coord in rest {
        let next = local32(extent, *coord);
        let _next = path.line_to(point(next[0], next[1]));
    }
    path.end(true);
}

fn local32(extent: u32, coordinate: Coord<i32>) -> [f32; 2] {
    let extent = extent as f32;
    [coordinate.x as f32 / extent, coordinate.y as f32 / extent]
}

fn world64(key: TileKey, extent: u32, coordinate: Coord<i32>) -> [f64; 2] {
    let scale = f64::from(1_u32 << key.zoom);
    let extent = f64::from(extent);
    [
        (f64::from(key.x) + f64::from(coordinate.x) / extent) / scale,
        (f64::from(key.y) + f64::from(coordinate.y) / extent) / scale,
    ]
}

fn join_normal(points: &[[f32; 2]], slot: usize) -> [f32; 2] {
    let prior = slot.saturating_sub(1);
    let next = (slot + 1).min(points.len() - 1);
    let incoming = direction(points[prior], points[slot]);
    let outgoing = direction(points[slot], points[next]);
    let first = if slot == 0 { outgoing } else { incoming };
    let second = if slot + 1 == points.len() {
        incoming
    } else {
        outgoing
    };
    let normal_a = [-first[1], first[0]];
    let normal_b = [-second[1], second[0]];
    let sum = [normal_a[0] + normal_b[0], normal_a[1] + normal_b[1]];
    let length = sum[0].hypot(sum[1]);
    if length <= f32::EPSILON {
        return normal_b;
    }
    let miter = [sum[0] / length, sum[1] / length];
    let divisor = miter[0].mul_add(normal_b[0], miter[1] * normal_b[1]);
    let reach = if divisor.abs() <= 0.25 {
        1.0
    } else {
        (1.0 / divisor).clamp(-3.0, 3.0)
    };
    [miter[0] * reach, miter[1] * reach]
}

fn direction(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    let delta = [b[0] - a[0], b[1] - a[1]];
    let length = delta[0].hypot(delta[1]);
    if length <= f32::EPSILON {
        [1.0, 0.0]
    } else {
        [delta[0] / length, delta[1] / length]
    }
}

fn purge_partials(directory: &FsPath) -> Result<()> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Ok(());
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let stale = path
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .is_some_and(|age| age >= Duration::from_hours(24));
        if path
            .extension()
            .is_some_and(|extension| extension == "partial")
            && stale
        {
            std::fs::remove_file(&path)
                .with_context(|| format!("remove stale basemap {}", path.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::{CacheClass, CacheManager};
    use fast_mvt::MvtTileBuilder;
    use pmtiles::{PmTilesWriter, TileType};
    use std::{
        fs::File,
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        thread::{self, JoinHandle},
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn cover_prefetches_without_duplicate_or_unbounded_source_demand() -> Result<()> {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let default_cover = cover(Viewport::default(), rect);
        assert_eq!(default_cover.strata.len(), 6);
        assert_eq!(default_cover.strata[4].intent, Intent::Required);
        assert_eq!(default_cover.strata[5].intent, Intent::Prefetch);
        assert_eq!(
            default_cover
                .finest_ready(|_| true)
                .map(|stratum| stratum.intent),
            Some(Intent::Required)
        );

        let wide = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 920.0));
        for zoom in [
            Viewport::MIN_ZOOM,
            Viewport::default().zoom,
            Viewport::MAX_ZOOM,
        ] {
            let cover = cover(
                Viewport {
                    zoom,
                    ..Viewport::default()
                },
                wide,
            );
            for stratum in cover.strata {
                let distinct = stratum
                    .keys
                    .iter()
                    .copied()
                    .collect::<std::collections::HashSet<_>>();
                assert_eq!(distinct.len(), stratum.keys.len());
                assert!(stratum.keys.iter().all(|tile| tile.zoom <= MAX_SOURCE_ZOOM));
            }
        }

        let view = Viewport {
            zoom: Viewport::MAX_ZOOM,
            ..Viewport::default()
        };
        let cover = cover(view, rect);
        let crown = cover.strata.last().context("top stratum")?;
        assert_eq!(crown.intent, Intent::Required);
        assert!(crown.keys.iter().all(|tile| tile.zoom == MAX_SOURCE_ZOOM));
        Ok(())
    }

    #[test]
    fn detail_demand_begins_at_native_zoom_only_inside_its_bounds() -> Result<()> {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let near = cover(view_at(-97.5, 38.5, 11.99), rect);
        assert!(
            near.strata
                .iter()
                .all(|stratum| stratum.keys.iter().all(|key| key.zoom <= LOCAL_MAX_ZOOM))
        );

        let detail = cover(view_at(-97.5, 38.5, 12.0), rect);
        let crown = detail.strata.last().context("detail stratum")?;
        assert_eq!(crown.intent, Intent::Required);
        assert!(crown.keys.iter().all(|key| key.zoom == MAX_SOURCE_ZOOM));

        let plains = required_keys(view_at(-97.5, 38.5, 12.0))
            .into_iter()
            .next()
            .context("plains tile")?;
        assert!(within_detail_bounds(plains));
        assert!(!within_detail_bounds(TileKey {
            zoom: MAX_SOURCE_ZOOM,
            x: 0,
            y: 0,
        }));
        Ok(())
    }

    #[test]
    fn detail_crosses_http_range_once_then_resides_in_cache() -> Result<()> {
        let root = test_root("detail-cache")?;
        let local_path = root.join("local.pmtiles");
        let remote_path = root.join("remote.pmtiles");
        let tile = MvtTileBuilder::new().layer("earth")?.end().encode();
        let key = required_keys(view_at(-97.5, 38.5, 12.0))
            .into_iter()
            .next()
            .context("detail tile")?;
        forge_archive(
            &local_path,
            TileKey {
                zoom: 0,
                x: 0,
                y: 0,
            },
            &tile,
        )?;
        forge_archive(&remote_path, key, &tile)?;

        let server = RangeServer::raise(std::fs::read(remote_path)?)?;
        let store = CacheManager::standard(root.join("cache")).store(CacheClass::Basemap);
        let detail = Detail::new(
            DetailSource {
                url: server.url(),
                generation: "20260809".to_owned(),
            },
            store.clone(),
        );
        let runtime = runtime()?;
        let local = runtime.block_on(Archive::new_with_cached_path(
            HashMapCache::default(),
            &local_path,
        ))?;

        let fetched = load_tile(&runtime, &local, Some(&detail), key)?.context("remote tile")?;
        assert_eq!(fetched.as_ref(), tile);
        let requests = server.requests();
        assert!(requests > 0);
        let cached = load_tile(&runtime, &local, Some(&detail), key)?.context("cached tile")?;
        assert_eq!(cached.as_ref(), tile);
        assert_eq!(server.requests(), requests);

        let blade = PathBuf::from("20260809")
            .join(MAX_SOURCE_ZOOM.to_string())
            .join(key.x.to_string())
            .join(format!("{}.mvt", key.y));
        assert_eq!(
            store.recall(&blade, valid_mvt)?.as_deref(),
            Some(tile.as_slice())
        );
        server.finish()?;
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    fn forge_archive(path: &FsPath, key: TileKey, tile: &[u8]) -> Result<()> {
        let file = File::create(path)?;
        let mut writer = PmTilesWriter::new(TileType::Mvt)
            .min_zoom(key.zoom)
            .max_zoom(key.zoom)
            .create(file)?;
        writer.add_tile(key.coordinate()?, tile)?;
        writer.finalize()?;
        Ok(())
    }

    fn test_root(name: &str) -> Result<PathBuf> {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let root = std::env::temp_dir().join(format!("hrrr-{name}-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&root)?;
        Ok(root)
    }

    struct RangeServer {
        address: std::net::SocketAddr,
        halt: Arc<AtomicBool>,
        requests: Arc<AtomicUsize>,
        faults: Receiver<String>,
        server: Arc<tiny_http::Server>,
        thread: Option<JoinHandle<()>>,
    }

    impl RangeServer {
        fn raise(bytes: Vec<u8>) -> Result<Self> {
            let server = Arc::new(
                tiny_http::Server::http(("127.0.0.1", 0))
                    .map_err(|error| anyhow::anyhow!("raise range fixture: {error}"))?,
            );
            let address = server
                .server_addr()
                .to_ip()
                .context("range fixture did not bind an IP socket")?;
            let halt = Arc::new(AtomicBool::new(false));
            let thread_halt = Arc::clone(&halt);
            let requests = Arc::new(AtomicUsize::new(0));
            let thread_requests = Arc::clone(&requests);
            let thread_server = Arc::clone(&server);
            let (fault_tx, faults) = bounded(1);
            let thread = thread::Builder::new()
                .name("pmtiles-range-fixture".to_owned())
                .spawn(move || {
                    while !thread_halt.load(Ordering::Acquire) {
                        match thread_server.recv_timeout(Duration::from_millis(10)) {
                            Ok(Some(request)) => {
                                if let Err(error) = serve_range(request, &bytes) {
                                    let _sent = fault_tx.try_send(format!("{error:#}"));
                                    return;
                                }
                                let _previous = thread_requests.fetch_add(1, Ordering::Relaxed);
                            }
                            Ok(None) => {}
                            Err(error) => {
                                let _sent = fault_tx.try_send(error.to_string());
                                return;
                            }
                        }
                    }
                })?;
            Ok(Self {
                address,
                halt,
                requests,
                faults,
                server,
                thread: Some(thread),
            })
        }

        fn url(&self) -> String {
            format!("http://{}/detail.pmtiles", self.address)
        }

        fn requests(&self) -> usize {
            self.requests.load(Ordering::Relaxed)
        }

        fn finish(mut self) -> Result<()> {
            self.stop();
            if let Ok(fault) = self.faults.try_recv() {
                anyhow::bail!("range fixture failed: {fault}");
            }
            Ok(())
        }

        fn stop(&mut self) {
            self.halt.store(true, Ordering::Release);
            self.server.unblock();
            if let Some(thread) = self.thread.take() {
                let _joined = thread.join();
            }
        }
    }

    impl Drop for RangeServer {
        fn drop(&mut self) {
            self.stop();
        }
    }

    fn serve_range(request: tiny_http::Request, bytes: &[u8]) -> Result<()> {
        let range = request
            .headers()
            .iter()
            .find(|header| header.field.equiv("Range"))
            .map(|header| header.value.as_str())
            .context("range request omitted Range header")?;
        let range = range
            .strip_prefix("bytes=")
            .context("malformed Range header")?;
        let (start, end) = range.split_once('-').context("malformed byte range")?;
        let start = start.parse::<usize>().context("invalid range start")?;
        let end = end.parse::<usize>().context("invalid range end")?;
        if start >= bytes.len() || end < start {
            anyhow::bail!("range {start}-{end} exceeds {} bytes", bytes.len());
        }
        let end = end.min(bytes.len() - 1);
        let body = &bytes[start..=end];
        let response = tiny_http::Response::from_data(body)
            .with_status_code(206)
            .with_header(fixture_header(
                "Content-Range",
                &format!("bytes {start}-{end}/{}", bytes.len()),
            )?)
            .with_header(fixture_header("Accept-Ranges", "bytes")?)
            .with_header(fixture_header("ETag", "\"fixture\"")?);
        request
            .respond(response)
            .context("respond to range request")
    }

    fn fixture_header(name: &str, value: &str) -> Result<tiny_http::Header> {
        tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes())
            .map_err(|()| anyhow::anyhow!("invalid fixture header {name}: {value}"))
    }

    #[test]
    fn line_join_stays_finite_at_reversals() {
        let points = [[0.0, 0.0], [1.0, 0.0], [0.0, 0.0]];
        assert!(
            join_normal(&points, 1)
                .iter()
                .all(|value| value.is_finite())
        );
    }

    #[test]
    fn source_min_zoom_overrides_cartographic_fallback() -> Result<()> {
        let style = road_style(Some("minor_road"), Some("residential"), Some(11.25), 11)
            .context("residential road style")?;
        assert_eq!(style.onset_zoom, 11.25);
        Ok(())
    }

    fn required_keys(view: Viewport) -> Vec<TileKey> {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 920.0));
        cover(view, rect)
            .strata
            .into_iter()
            .find(|stratum| stratum.intent == Intent::Required)
            .map_or_else(Vec::new, |stratum| stratum.keys)
    }

    fn view_at(longitude: f64, latitude: f64, zoom: f64) -> Viewport {
        let x = (longitude + 180.0) / 360.0;
        let y = (1.0 - (latitude.to_radians().tan().asinh() / std::f64::consts::PI)) * 0.5;
        Viewport {
            center_mercator: [x, y],
            zoom,
        }
    }
}
