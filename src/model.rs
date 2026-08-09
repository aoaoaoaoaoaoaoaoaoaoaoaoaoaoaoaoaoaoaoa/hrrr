use anyhow::{Context as _, Result, bail};
use jiff::{Timestamp, tz::TimeZone};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, sync::Arc};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct GribLaw {
    pub template: u16,
    pub category: u8,
    pub parameter: u8,
    pub surface: FixedSurfaceLaw,
    pub time: GribTimeLaw,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FixedSurfaceLaw {
    pub kind: u8,
    pub metres: i32,
}

impl FixedSurfaceLaw {
    const GROUND: Self = Self { kind: 1, metres: 0 };
    const ENTIRE_ATMOSPHERE: Self = Self {
        kind: 10,
        metres: 0,
    };

    const fn metres_above_ground(metres: i32) -> Self {
        Self { kind: 103, metres }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GribTimeLaw {
    Instant,
    AccumulationFromRun,
    HourlyAccumulation,
}

impl GribLaw {
    const fn instant(category: u8, parameter: u8, surface: FixedSurfaceLaw) -> Self {
        Self {
            template: 0,
            category,
            parameter,
            surface,
            time: GribTimeLaw::Instant,
        }
    }

    const fn accumulation(time: GribTimeLaw) -> Self {
        Self {
            template: 8,
            category: 1,
            parameter: 8,
            surface: FixedSurfaceLaw::GROUND,
            time,
        }
    }
}

#[derive(Clone, Copy)]
enum InventoryLaw {
    AccumulationFromRun,
    HourlyAccumulation,
    Contains(&'static str),
}

impl InventoryLaw {
    fn matches(self, descriptor: &str) -> bool {
        match self {
            Self::AccumulationFromRun => {
                AccumulationWindow::parse(descriptor).is_some_and(AccumulationWindow::begins_at_run)
            }
            Self::HourlyAccumulation => {
                AccumulationWindow::parse(descriptor).is_some_and(AccumulationWindow::is_hourly)
            }
            Self::Contains(needle) => descriptor.contains(needle),
        }
    }
}

#[derive(Clone, Copy)]
struct ProductLaw {
    label: &'static str,
    cache_name: &'static str,
    inventory: InventoryLaw,
    grib: GribLaw,
}

macro_rules! field_arsenal {
    (
        $(
            [
                $(
                    $(#[$attribute:meta])*
                    $variant:ident {
                        label: $label:literal,
                        cache: $cache:literal,
                        inventory: $inventory:expr,
                        grib: $grib:expr $(,)?
                    }
                ),+ $(,)?
            ]
        ),+ $(,)?
    ) => {
        #[derive(
            Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd,
            Serialize,
        )]
        #[serde(rename_all = "snake_case")]
        pub enum Product {
            $(
                $(
                    $(#[$attribute])*
                    $variant,
                )+
            )+
        }

        impl Product {
            pub const ALL: [Self; field_arsenal!(@count $($($variant),+),+)] = [
                $($(Self::$variant),+),+
            ];
            pub const ROWS: &'static [&'static [Self]] = &[
                $(&[$(Self::$variant),+]),+
            ];

            const fn law(self) -> ProductLaw {
                match self {
                    $(
                        $(
                            Self::$variant => ProductLaw {
                                label: $label,
                                cache_name: $cache,
                                inventory: $inventory,
                                grib: $grib,
                            },
                        )+
                    )+
                }
            }

            pub const fn label(self) -> &'static str {
                self.law().label
            }

            pub const fn cache_name(self) -> &'static str {
                self.law().cache_name
            }

            pub(crate) fn index_match(self, descriptor: &str) -> bool {
                self.law().inventory.matches(descriptor)
            }

            pub(crate) const fn grib_law(self) -> GribLaw {
                self.law().grib
            }
        }
    };
    (@count $($variant:ident),+ $(,)?) => {
        0_usize $(+ field_arsenal!(@one $variant))+
    };
    (@one $variant:ident) => {{
        let _ = stringify!($variant);
        1_usize
    }};
}

field_arsenal! {
    [
        #[serde(alias = "qpf")]
        QpfRun {
            label: "QPF · TOTAL",
            cache: "qpf",
            inventory: InventoryLaw::AccumulationFromRun,
            grib: GribLaw::accumulation(GribTimeLaw::AccumulationFromRun),
        },
        QpfHour {
            label: "QPF · 1 HOUR",
            cache: "qpf-hour",
            inventory: InventoryLaw::HourlyAccumulation,
            grib: GribLaw::accumulation(GribTimeLaw::HourlyAccumulation),
        },
    ],
    [
        #[default]
        Smoke {
            label: "SURFACE SMOKE · 8 M AGL",
            cache: "smoke",
            inventory: InventoryLaw::Contains(":MASSDEN:8 m above ground:"),
            grib: GribLaw::instant(20, 0, FixedSurfaceLaw::metres_above_ground(8)),
        },
    ],
    [
        Temperature {
            label: "TEMPERATURE · 2 M AGL",
            cache: "temperature",
            inventory: InventoryLaw::Contains(":TMP:2 m above ground:"),
            grib: GribLaw::instant(0, 0, FixedSurfaceLaw::metres_above_ground(2)),
        },
    ],
    [
        CloudCover {
            label: "CLOUD COVER",
            cache: "cloud-cover",
            inventory: InventoryLaw::Contains(":TCDC:entire atmosphere:"),
            grib: GribLaw::instant(6, 1, FixedSurfaceLaw::ENTIRE_ATMOSPHERE),
        },
    ],
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Overlay(Option<Product>);

impl Overlay {
    pub const fn active(self) -> Option<Product> {
        self.0
    }

    pub fn strike(self, product: Product) -> Self {
        Self((self.0 != Some(product)).then_some(product))
    }
}

impl From<Product> for Overlay {
    fn from(product: Product) -> Self {
        Self(Some(product))
    }
}

impl Serialize for Overlay {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self.0 {
            Some(product) => product.serialize(serializer),
            None => serializer.serialize_str("none"),
        }
    }
}

impl<'de> Deserialize<'de> for Overlay {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum BareMap {
            None,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            Field(Product),
            Bare(BareMap),
        }

        Ok(match Wire::deserialize(deserializer)? {
            Wire::Field(product) => product.into(),
            Wire::Bare(BareMap::None) => Self::default(),
        })
    }
}

#[derive(Clone, Copy)]
struct AccumulationWindow {
    start_hour: u16,
    end_hour: u16,
}

impl AccumulationWindow {
    fn parse(descriptor: &str) -> Option<Self> {
        let (_, tail) = descriptor.split_once(":APCP:surface:")?;
        let (span, multiplier) = tail
            .strip_suffix(" hour acc fcst:")
            .map(|span| (span, 1))
            .or_else(|| tail.strip_suffix(" day acc fcst:").map(|span| (span, 24)))?;
        let (start, end) = span.split_once('-')?;
        let start_hour = start.parse::<u16>().ok()?.checked_mul(multiplier)?;
        let end_hour = end.parse::<u16>().ok()?.checked_mul(multiplier)?;
        (start_hour <= end_hour).then_some(Self {
            start_hour,
            end_hour,
        })
    }

    const fn begins_at_run(self) -> bool {
        self.start_hour == 0
    }

    const fn is_hourly(self) -> bool {
        self.end_hour == 0 || self.end_hour - self.start_hour == 1
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RunSelection {
    #[default]
    Latest,
    LatestLong,
    Fixed(RunId),
}

impl RunSelection {
    pub fn bind(self, latest: RunId) -> RunId {
        match self {
            Self::Latest => latest,
            Self::LatestLong => latest.latest_extended_at_or_before().unwrap_or(latest),
            Self::Fixed(run) => run.min(latest),
        }
    }

    pub const fn fixed(self) -> Option<RunId> {
        match self {
            Self::Fixed(run) => Some(run),
            Self::Latest | Self::LatestLong => None,
        }
    }

    pub fn rectify(self, latest: RunId) -> Self {
        match self {
            Self::Fixed(run) if run > latest => Self::Latest,
            _ => self,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(try_from = "i64", into = "i64")]
pub struct RunId(i64);

impl RunId {
    pub fn forge(epoch_second: i64) -> Result<Self> {
        if epoch_second.rem_euclid(3_600) != 0 {
            bail!("HRRR cycle {epoch_second} is not aligned to an hour");
        }
        let _timestamp =
            Timestamp::from_second(epoch_second).context("HRRR cycle lies outside civil time")?;
        Ok(Self(epoch_second))
    }

    pub fn hourly_at_or_before(timestamp: Timestamp) -> Self {
        Self(timestamp.as_second().div_euclid(3_600) * 3_600)
    }

    pub fn hours_ago(self, hours: u8) -> Self {
        Self(self.0 - i64::from(hours) * 3_600)
    }

    pub fn hours_after(self, hours: u8) -> Self {
        Self(self.0 + i64::from(hours) * 3_600)
    }

    pub fn timestamp(self) -> Result<Timestamp> {
        Timestamp::from_second(self.0).map_err(Into::into)
    }

    pub fn valid_timestamp(self, lead: LeadHour) -> Result<Timestamp> {
        Timestamp::from_second(self.0 + i64::from(lead.get()) * 3_600).map_err(Into::into)
    }

    pub fn rebase_lead(self, source: Self, source_lead: LeadHour, frontier: LeadHour) -> LeadHour {
        let valid = source
            .0
            .saturating_add(i64::from(source_lead.get()) * 3_600);
        let hours = valid
            .saturating_sub(self.0)
            .div_euclid(3_600)
            .clamp(0, i64::from(frontier.get()));
        LeadHour(hours as u8)
    }

    pub fn valid_month_utc(self, lead: LeadHour) -> Result<i8> {
        Ok(self.valid_timestamp(lead)?.to_zoned(TimeZone::UTC).month())
    }

    pub fn stamp(self) -> Result<String> {
        Ok(self.timestamp()?.strftime("%Y%m%d%H").to_string())
    }

    pub fn date(self) -> Result<String> {
        Ok(self.timestamp()?.strftime("%Y%m%d").to_string())
    }

    pub fn cycle(self) -> Result<u8> {
        let hour = self.timestamp()?.to_zoned(TimeZone::UTC).hour();
        u8::try_from(hour).map_err(Into::into)
    }

    pub fn horizon(self) -> Result<LeadHour> {
        LeadHour::forge(if self.cycle()?.is_multiple_of(6) {
            48
        } else {
            18
        })
    }

    pub fn latest_extended_at_or_before(self) -> Result<Self> {
        Ok(self.hours_ago(self.cycle()? % 6))
    }

    pub fn local_label(self) -> Result<String> {
        Ok(self
            .timestamp()?
            .to_zoned(TimeZone::system())
            .strftime("%a %b %e · %I %p %Z")
            .to_string())
    }

    pub fn valid_local_label(self, lead: LeadHour) -> Result<String> {
        Ok(self
            .valid_timestamp(lead)?
            .to_zoned(TimeZone::system())
            .strftime("%a %b %e · %I:%M %p %Z")
            .to_string())
    }
}

impl TryFrom<i64> for RunId {
    type Error = String;

    fn try_from(epoch_second: i64) -> Result<Self, Self::Error> {
        Self::forge(epoch_second).map_err(|error| error.to_string())
    }
}

impl From<RunId> for i64 {
    fn from(run: RunId) -> Self {
        run.0
    }
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(try_from = "u8", into = "u8")]
pub struct LeadHour(u8);

impl LeadHour {
    pub const ZERO: Self = Self(0);
    pub const MAX: u8 = 48;

    pub fn forge(hour: u8) -> Result<Self> {
        if hour > Self::MAX {
            bail!("forecast lead {hour} exceeds HRRR ceiling {}", Self::MAX);
        }
        Ok(Self(hour))
    }

    pub const fn get(self) -> u8 {
        self.0
    }

    pub fn saturating_next(self, horizon: Self) -> Self {
        Self(self.0.saturating_add(1).min(horizon.0))
    }

    pub fn saturating_previous(self) -> Self {
        Self(self.0.saturating_sub(1))
    }
}

impl TryFrom<u8> for LeadHour {
    type Error = String;

    fn try_from(hour: u8) -> Result<Self, Self::Error> {
        Self::forge(hour).map_err(|error| error.to_string())
    }
}

impl From<LeadHour> for u8 {
    fn from(lead: LeadHour) -> Self {
        lead.0
    }
}

impl fmt::Display for LeadHour {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "F{:02}", self.0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RunExtent {
    run: RunId,
    published: LeadHour,
}

impl RunExtent {
    pub fn forge(run: RunId, published: LeadHour) -> Result<Self> {
        let horizon = run.horizon()?;
        if published > horizon {
            bail!("published lead {published} exceeds {run:?} horizon {horizon}");
        }
        Ok(Self { run, published })
    }

    pub const fn run(self) -> RunId {
        self.run
    }

    pub const fn published(self) -> LeadHour {
        self.published
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FrameKey {
    pub run: RunId,
    pub lead: LeadHour,
    pub product: Product,
}

/// The spherical Lambert conformal law carried by each HRRR GRIB message.
/// Keeping it beside the values prevents a visually plausible but displaced
/// field if NOAA ever changes the grid definition.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LambertGrid {
    pub cone: f64,
    pub radius_factor: f64,
    pub origin_rho: f64,
    pub central_lon: f64,
    pub first_xy: [f64; 2],
    pub spacing: [f64; 2],
}

impl LambertGrid {
    pub fn forge(
        radius: f64,
        first_lat: f64,
        first_lon: f64,
        origin_lat: f64,
        central_lon: f64,
        standard_parallels: [f64; 2],
        spacing: [f64; 2],
    ) -> Result<Self> {
        let radians = f64::to_radians;
        let [parallel_a, parallel_b] = standard_parallels.map(radians);
        let cone = if (parallel_a - parallel_b).abs() < 1.0e-12 {
            parallel_a.sin()
        } else {
            (parallel_a.cos() / parallel_b.cos()).ln()
                / ((std::f64::consts::FRAC_PI_4 + parallel_b * 0.5).tan()
                    / (std::f64::consts::FRAC_PI_4 + parallel_a * 0.5).tan())
                .ln()
        };
        if radius <= 0.0 || cone.abs() < 1.0e-12 || spacing.iter().any(|v| *v <= 0.0) {
            bail!("degenerate Lambert conformal grid definition");
        }
        let radius_factor = radius
            * parallel_a.cos()
            * (std::f64::consts::FRAC_PI_4 + parallel_a * 0.5)
                .tan()
                .powf(cone)
            / cone;
        let central_lon = radians(central_lon);
        let rho = |latitude: f64| {
            radius_factor
                / (std::f64::consts::FRAC_PI_4 + radians(latitude) * 0.5)
                    .tan()
                    .powf(cone)
        };
        let origin_rho = rho(origin_lat);
        let first_rho = rho(first_lat);
        let delta = radians(first_lon) - central_lon;
        let theta = cone * delta.sin().atan2(delta.cos());
        let first_xy = [
            first_rho * theta.sin(),
            origin_rho - first_rho * theta.cos(),
        ];
        Ok(Self {
            cone,
            radius_factor,
            origin_rho,
            central_lon,
            first_xy,
            spacing,
        })
    }

    pub fn grid_at_lon_lat(self, longitude: f64, latitude: f64) -> [f64; 2] {
        let latitude = latitude.to_radians();
        let longitude = longitude.to_radians();
        let rho = self.radius_factor
            / (std::f64::consts::FRAC_PI_4 + latitude * 0.5)
                .tan()
                .powf(self.cone);
        let delta = longitude - self.central_lon;
        let theta = self.cone * delta.sin().atan2(delta.cos());
        let x = rho * theta.sin();
        let y = self.origin_rho - rho * theta.cos();
        [
            (x - self.first_xy[0]) / self.spacing[0],
            (y - self.first_xy[1]) / self.spacing[1],
        ]
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Viewport {
    /// Web Mercator world coordinates, each nominally in `0..=1`.
    pub center_mercator: [f64; 2],
    /// Fractional slippy-map zoom. Tiles may stop; the field does not.
    pub zoom: f64,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            center_mercator: [0.229_166_666_666_666_67, 0.383_960_077_341_341_9],
            zoom: 4.7,
        }
    }
}

impl Viewport {
    pub const MIN_ZOOM: f64 = 1.0;
    pub const MAX_ZOOM: f64 = 24.0;

    pub fn normalize(&mut self) {
        if !self.center_mercator.iter().all(|v| v.is_finite()) {
            self.center_mercator = Self::default().center_mercator;
        }
        if !self.zoom.is_finite() {
            self.zoom = Self::default().zoom;
        }
        self.center_mercator[0] = self.center_mercator[0].rem_euclid(1.0);
        self.center_mercator[1] = self.center_mercator[1].clamp(0.0, 1.0);
        self.zoom = self.zoom.clamp(Self::MIN_ZOOM, Self::MAX_ZOOM);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct MercatorPoint([f64; 2]);

#[derive(Deserialize)]
#[serde(untagged)]
enum PointWire {
    Pair([f64; 2]),
    NamedPin(NamedPinWire),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NamedPinWire {
    world: [f64; 2],
    #[serde(default)]
    name: Option<String>,
}

impl<'de> Deserialize<'de> for MercatorPoint {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let world = match PointWire::deserialize(deserializer)? {
            PointWire::Pair(world) => world,
            PointWire::NamedPin(pin) => {
                let _discarded_name = pin.name;
                pin.world
            }
        };
        Self::forge(world).ok_or_else(|| serde::de::Error::custom("map coordinate is not finite"))
    }
}

impl MercatorPoint {
    pub fn forge(world: [f64; 2]) -> Option<Self> {
        let mut world = world;
        if !world.iter().all(|value| value.is_finite()) {
            return None;
        }
        world[0] = world[0].rem_euclid(1.0);
        world[1] = world[1].clamp(0.0, 1.0);
        Some(Self(world))
    }

    pub const fn world(self) -> [f64; 2] {
        self.0
    }

    pub fn shifted(self, delta: [f64; 2]) -> Self {
        assert!(
            delta.iter().all(|value| value.is_finite()),
            "map displacement must be finite"
        );
        Self([
            (self.0[0] + delta[0]).rem_euclid(1.0),
            (self.0[1] + delta[1]).clamp(0.0, 1.0),
        ])
    }

    pub fn normalize(self) -> Option<Self> {
        Self::forge(self.0)
    }
}

#[derive(Clone, Debug)]
pub struct FieldGrid {
    pub values: Arc<[f32]>,
    pub width: u32,
    pub height: u32,
    pub projection: LambertGrid,
}

impl FieldGrid {
    pub fn forge(
        values: Vec<f32>,
        width: usize,
        height: usize,
        projection: LambertGrid,
    ) -> Result<Self> {
        if values.len() != width.saturating_mul(height) {
            bail!(
                "decoded field has {} values for {width}×{height} grid",
                values.len()
            );
        }
        let width = u32::try_from(width)?;
        let height = u32::try_from(height)?;
        Ok(Self {
            values: values.into(),
            width,
            height,
            projection,
        })
    }

    pub fn at(&self, i: u32, j: u32) -> Option<f32> {
        if i >= self.width || j >= self.height {
            return None;
        }
        self.values.get((j * self.width + i) as usize).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[derive(Debug, Deserialize, Serialize)]
    struct PinCase {
        pins: Vec<MercatorPoint>,
    }

    #[test]
    fn overlay_strikes_select_switch_and_deselect() {
        let bare = Overlay::default();
        let smoke = bare.strike(Product::Smoke);
        assert_eq!(bare.active(), None);
        assert_eq!(smoke.active(), Some(Product::Smoke));
        assert_eq!(smoke.strike(Product::Smoke), bare);
        assert_eq!(
            smoke.strike(Product::Temperature).active(),
            Some(Product::Temperature)
        );
    }

    #[test]
    fn field_arsenal_is_bijective_across_layout_and_cache() {
        let rows = Product::ROWS
            .iter()
            .flat_map(|row| row.iter().copied())
            .collect::<Vec<_>>();
        assert_eq!(rows, Product::ALL);
        assert_eq!(
            Product::ALL
                .iter()
                .map(|product| product.cache_name())
                .collect::<BTreeSet<_>>()
                .len(),
            Product::ALL.len()
        );
    }

    #[test]
    fn extended_cycles_own_the_long_horizon() -> Result<()> {
        let run = RunId::hourly_at_or_before(Timestamp::from_second(1_752_926_400)?);
        assert_eq!(run.cycle()?, 12);
        assert_eq!(run.horizon()?.get(), 48);
        assert_eq!(run.hours_ago(1).horizon()?.get(), 18);
        assert_eq!(run.latest_extended_at_or_before()?, run);
        assert_eq!(run.hours_after(5).latest_extended_at_or_before()?, run);
        assert_eq!(RunSelection::LatestLong.bind(run.hours_after(5)), run);
        assert_eq!(
            RunSelection::Fixed(run.hours_ago(1)).bind(run),
            run.hours_ago(1)
        );
        Ok(())
    }

    #[test]
    fn rebased_runs_preserve_valid_time_then_saturate_at_their_edges() -> Result<()> {
        let source = RunId::hourly_at_or_before(Timestamp::from_second(1_752_926_400)?);
        let source_lead = LeadHour::forge(10)?;
        let frontier = LeadHour::forge(18)?;

        assert_eq!(
            source
                .hours_after(6)
                .rebase_lead(source, source_lead, frontier),
            LeadHour::forge(4)?
        );
        assert_eq!(
            source
                .hours_after(12)
                .rebase_lead(source, source_lead, frontier),
            LeadHour::ZERO
        );
        assert_eq!(
            source
                .hours_ago(12)
                .rebase_lead(source, source_lead, frontier),
            frontier
        );
        Ok(())
    }

    #[test]
    fn run_extents_cannot_breach_their_cycle_horizon() -> Result<()> {
        let run = RunId::hourly_at_or_before(Timestamp::from_second(1_752_926_400)?).hours_ago(1);
        assert_eq!(
            RunExtent::forge(run, LeadHour::forge(18)?)?
                .published()
                .get(),
            18
        );
        assert!(RunExtent::forge(run, LeadHour::forge(19)?).is_err());
        Ok(())
    }

    #[test]
    fn viewport_repels_corruption() {
        let mut view = Viewport {
            center_mercator: [f64::NAN, 4.0],
            zoom: f64::INFINITY,
        };
        view.normalize();
        assert_eq!(view, Viewport::default());
    }

    #[test]
    fn lambert_longitudes_cross_the_antimeridian_encoding() -> Result<()> {
        let grid = LambertGrid::forge(
            6_371_229.0,
            21.138,
            237.28,
            38.5,
            262.5,
            [38.5, 38.5],
            [3_000.0, 3_000.0],
        )?;
        let west = grid.grid_at_lon_lat(-97.5, 38.5);
        let east = grid.grid_at_lon_lat(262.5, 38.5);
        assert!(
            west.into_iter()
                .zip(east)
                .all(|(a, b)| (a - b).abs() < 1.0e-9)
        );
        assert!((0.0..1_799.0).contains(&west[0]));
        assert!((0.0..1_059.0).contains(&west[1]));
        Ok(())
    }

    #[test]
    fn pins_contract_legacy_coordinates_and_named_records() -> Result<()> {
        let pair: PinCase = toml::from_str("pins = [[0.25, 0.4]]")?;
        assert_eq!(pair.pins[0].world(), [0.25, 0.4]);

        let named: PinCase = toml::from_str(
            r#"
            [[pins]]
            world = [0.3, 0.6]
            name = "western fire line"
            "#,
        )?;
        assert_eq!(named.pins[0].world(), [0.3, 0.6]);

        let encoded = toml::to_string(&PinCase { pins: named.pins })?;
        assert!(encoded.contains("pins = [[0.3, 0.6]]"));
        assert!(!encoded.contains("name"));
        Ok(())
    }
}
