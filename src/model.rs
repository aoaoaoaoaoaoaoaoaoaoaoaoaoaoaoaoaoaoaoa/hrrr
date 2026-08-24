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
    pub scale_factor: i8,
    pub scaled_value: i32,
}

impl FixedSurfaceLaw {
    const GROUND: Self = Self {
        kind: 1,
        scale_factor: 0,
        scaled_value: 0,
    };
    const ENTIRE_ATMOSPHERE: Self = Self {
        kind: 10,
        scale_factor: 0,
        scaled_value: 0,
    };
    const SIGMA_ONE: Self = Self {
        kind: 104,
        scale_factor: 4,
        scaled_value: 10_000,
    };

    const fn metres_above_ground(metres: i32) -> Self {
        Self {
            kind: 103,
            scale_factor: 0,
            scaled_value: metres,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GribTimeLaw {
    Instant,
    AccumulationFromRun,
    HourlyAccumulation,
    DailySummary { start_shift: i8, end_shift: i8 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TemporalShape {
    Instant,
    Interval,
    Cumulative,
}

impl GribTimeLaw {
    const fn temporal_shape(self) -> TemporalShape {
        match self {
            Self::Instant => TemporalShape::Instant,
            Self::HourlyAccumulation => TemporalShape::Interval,
            Self::DailySummary { .. } => TemporalShape::Interval,
            Self::AccumulationFromRun => TemporalShape::Cumulative,
        }
    }
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

    const fn daily_summary(category: u8, parameter: u8, start_shift: i8, end_shift: i8) -> Self {
        Self {
            template: 8,
            category,
            parameter,
            surface: FixedSurfaceLaw::SIGMA_ONE,
            time: GribTimeLaw::DailySummary {
                start_shift,
                end_shift,
            },
        }
    }
}

#[derive(Clone, Copy)]
enum InventoryLaw {
    AccumulationFromRun,
    HourlyAccumulation,
    Contains(&'static str),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AqmBundle {
    FineParticulate,
    OzoneEightHour,
    OzoneOneHour,
}

impl AqmBundle {
    pub(crate) const DAY_SLOTS: u8 = 3;
    pub(crate) const ALL: [Self; 3] = [
        Self::FineParticulate,
        Self::OzoneEightHour,
        Self::OzoneOneHour,
    ];

    pub(crate) const fn file_stem(self) -> &'static str {
        match self {
            Self::FineParticulate => "ave_24hr_pm25_bc",
            Self::OzoneEightHour => "max_8hr_o3_bc",
            Self::OzoneOneHour => "max_1hr_o3_bc",
        }
    }
}

#[derive(Clone, Copy)]
enum AcquisitionLaw {
    Hrrr(InventoryLaw),
    Aqm(AqmBundle),
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum Ingredient {
    Scalar,
    Eastward,
    Northward,
    FineParticulate,
    OzoneEightHour,
    OzoneOneHour,
}

const SCALAR_INGREDIENTS: &[Ingredient] = &[Ingredient::Scalar];
const VECTOR_INGREDIENTS: &[Ingredient] = &[Ingredient::Eastward, Ingredient::Northward];
const AIR_QUALITY_INGREDIENTS: &[Ingredient] = &[
    Ingredient::FineParticulate,
    Ingredient::OzoneEightHour,
    Ingredient::OzoneOneHour,
];

#[derive(Clone, Copy)]
struct IngredientLaw {
    cache_suffix: &'static str,
    acquisition: AcquisitionLaw,
    grib: GribLaw,
}

impl IngredientLaw {
    const fn hrrr(cache_suffix: &'static str, inventory: InventoryLaw, grib: GribLaw) -> Self {
        Self {
            cache_suffix,
            acquisition: AcquisitionLaw::Hrrr(inventory),
            grib,
        }
    }

    const fn aqm(cache_suffix: &'static str, bundle: AqmBundle, grib: GribLaw) -> Self {
        Self {
            cache_suffix,
            acquisition: AcquisitionLaw::Aqm(bundle),
            grib,
        }
    }
}

#[derive(Clone, Copy)]
enum FieldRecipe {
    Scalar(IngredientLaw),
    Vector {
        eastward: IngredientLaw,
        northward: IngredientLaw,
    },
    AirQuality {
        fine_particulate: IngredientLaw,
        ozone_eight_hour: IngredientLaw,
        ozone_one_hour: IngredientLaw,
    },
}

impl FieldRecipe {
    const fn scalar(inventory: InventoryLaw, grib: GribLaw) -> Self {
        Self::Scalar(IngredientLaw::hrrr("", inventory, grib))
    }

    const fn ingredients(self) -> &'static [Ingredient] {
        match self {
            Self::Scalar(_) => SCALAR_INGREDIENTS,
            Self::Vector { .. } => VECTOR_INGREDIENTS,
            Self::AirQuality { .. } => AIR_QUALITY_INGREDIENTS,
        }
    }

    const fn law(self, ingredient: Ingredient) -> Option<IngredientLaw> {
        match (self, ingredient) {
            (Self::Scalar(law), Ingredient::Scalar) => Some(law),
            (Self::Vector { eastward, .. }, Ingredient::Eastward) => Some(eastward),
            (Self::Vector { northward, .. }, Ingredient::Northward) => Some(northward),
            (
                Self::AirQuality {
                    fine_particulate, ..
                },
                Ingredient::FineParticulate,
            ) => Some(fine_particulate),
            (
                Self::AirQuality {
                    ozone_eight_hour, ..
                },
                Ingredient::OzoneEightHour,
            ) => Some(ozone_eight_hour),
            (Self::AirQuality { ozone_one_hour, .. }, Ingredient::OzoneOneHour) => {
                Some(ozone_one_hour)
            }
            _ => None,
        }
    }

    const fn shape(self) -> FieldShape {
        match self {
            Self::Scalar(_) => FieldShape::Scalar,
            Self::Vector { .. } => FieldShape::Vector,
            Self::AirQuality { .. } => FieldShape::AirQuality,
        }
    }

    const fn temporal_shape(self) -> TemporalShape {
        let law = match self {
            Self::Scalar(law)
            | Self::Vector { eastward: law, .. }
            | Self::AirQuality {
                fine_particulate: law,
                ..
            } => law,
        };
        law.grib.time.temporal_shape()
    }

    const fn system(self) -> ForecastSystem {
        match self {
            Self::Scalar(_) | Self::Vector { .. } => ForecastSystem::Hrrr,
            Self::AirQuality { .. } => ForecastSystem::Aqm,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FieldShape {
    Scalar,
    Vector,
    AirQuality,
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
    recipe: FieldRecipe,
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
                        recipe: $recipe:expr $(,)?
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
                                recipe: $recipe,
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

            pub(crate) const fn ingredients(self) -> &'static [Ingredient] {
                self.law().recipe.ingredients()
            }

            pub(crate) const fn shape(self) -> FieldShape {
                self.law().recipe.shape()
            }

            const fn ingredient_law(self, ingredient: Ingredient) -> Option<IngredientLaw> {
                self.law().recipe.law(ingredient)
            }

            pub const fn temporal_shape(self) -> TemporalShape {
                self.law().recipe.temporal_shape()
            }

            pub(crate) const fn system(self) -> ForecastSystem {
                self.law().recipe.system()
            }

            pub const fn has_baseline(self) -> bool {
                matches!(self.temporal_shape(), TemporalShape::Cumulative)
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
            recipe: FieldRecipe::scalar(
                InventoryLaw::AccumulationFromRun,
                GribLaw::accumulation(GribTimeLaw::AccumulationFromRun),
            ),
        },
        QpfHour {
            label: "QPF · 1 HOUR",
            cache: "qpf-hour",
            recipe: FieldRecipe::scalar(
                InventoryLaw::HourlyAccumulation,
                GribLaw::accumulation(GribTimeLaw::HourlyAccumulation),
            ),
        },
    ],
    [
        #[default]
        Smoke {
            label: "SURFACE SMOKE · 8 M AGL",
            cache: "smoke",
            recipe: FieldRecipe::scalar(
                InventoryLaw::Contains(":MASSDEN:8 m above ground:"),
                GribLaw::instant(20, 0, FixedSurfaceLaw::metres_above_ground(8)),
            ),
        },
    ],
    [
        Temperature {
            label: "TEMPERATURE",
            cache: "temperature",
            recipe: FieldRecipe::scalar(
                InventoryLaw::Contains(":TMP:2 m above ground:"),
                GribLaw::instant(0, 0, FixedSurfaceLaw::metres_above_ground(2)),
            ),
        },
        DewPoint {
            label: "DEW POINT",
            cache: "dew-point",
            recipe: FieldRecipe::scalar(
                InventoryLaw::Contains(":DPT:2 m above ground:"),
                GribLaw::instant(0, 6, FixedSurfaceLaw::metres_above_ground(2)),
            ),
        },
    ],
    [
        CloudCover {
            label: "CLOUD COVER",
            cache: "cloud-cover",
            recipe: FieldRecipe::scalar(
                InventoryLaw::Contains(":TCDC:entire atmosphere:"),
                GribLaw::instant(6, 1, FixedSurfaceLaw::ENTIRE_ATMOSPHERE),
            ),
        },
    ],
    [
        Wind {
            label: "WIND · 10 M AGL",
            cache: "wind",
            recipe: FieldRecipe::Vector {
                eastward: IngredientLaw::hrrr(
                    "eastward",
                    InventoryLaw::Contains(":UGRD:10 m above ground:"),
                    GribLaw::instant(2, 2, FixedSurfaceLaw::metres_above_ground(10)),
                ),
                northward: IngredientLaw::hrrr(
                    "northward",
                    InventoryLaw::Contains(":VGRD:10 m above ground:"),
                    GribLaw::instant(2, 3, FixedSurfaceLaw::metres_above_ground(10)),
                ),
            },
        },
    ],
    [
        AirQuality {
            label: "AIR QUALITY · AQI",
            cache: "air-quality",
            recipe: FieldRecipe::AirQuality {
                fine_particulate: IngredientLaw::aqm(
                    "pm25",
                    AqmBundle::FineParticulate,
                    GribLaw::daily_summary(13, 193, 0, 0),
                ),
                ozone_eight_hour: IngredientLaw::aqm(
                    "ozone-8h",
                    AqmBundle::OzoneEightHour,
                    GribLaw::daily_summary(14, 201, 6, 8),
                ),
                ozone_one_hour: IngredientLaw::aqm(
                    "ozone-1h",
                    AqmBundle::OzoneOneHour,
                    GribLaw::daily_summary(14, 200, 0, 1),
                ),
            },
        },
    ],
}

impl Product {
    pub(crate) fn horizon(self, run: ForecastRun) -> Result<LeadHour> {
        if self.system() != run.system {
            bail!("forecast product and run belong to different systems");
        }
        match self {
            Self::AirQuality => {
                let final_hour =
                    i16::from(aqm_day_zero(run.id)?) + i16::from(AqmBundle::DAY_SLOTS) * 24 - 1;
                LeadHour::forge(u8::try_from(final_hour)?)
            }
            _ => run.horizon(),
        }
    }

    fn canonical_lead(self, run: ForecastRun, lead: LeadHour) -> Option<LeadHour> {
        if self != Self::AirQuality {
            return Some(lead);
        }
        let slot = self.daily_slot(run, lead)?;
        let end = i16::from(aqm_day_zero(run.id).ok()?) + i16::from(slot) * 24 + 23;
        LeadHour::forge(u8::try_from(end.max(0)).ok()?).ok()
    }

    fn daily_slot(self, run: ForecastRun, lead: LeadHour) -> Option<u8> {
        if self != Self::AirQuality || run.system != ForecastSystem::Aqm {
            return None;
        }
        let day_zero = i16::from(aqm_day_zero(run.id).ok()?);
        let slot = (i16::from(lead.get()) - day_zero)
            .div_euclid(24)
            .clamp(0, i16::from(AqmBundle::DAY_SLOTS - 1));
        u8::try_from(slot).ok()
    }

    pub(crate) fn lead_axis(self, run: ForecastRun) -> Result<LeadAxis> {
        LeadAxis::forge(run, self)
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct LeadAxis {
    run: ForecastRun,
    product: Product,
    horizon: LeadHour,
}

#[derive(Clone, Copy)]
struct LeadDetent {
    valid_from: LeadHour,
    valid_through: LeadHour,
}

impl LeadAxis {
    fn forge(run: ForecastRun, product: Product) -> Result<Self> {
        Ok(Self {
            run,
            product,
            horizon: product.horizon(run)?,
        })
    }

    fn detents(self) -> impl Iterator<Item = LeadDetent> {
        (0..=self.horizon.get()).filter_map(move |hour| {
            let valid_from = LeadHour(hour);
            let valid_through = self.product.canonical_lead(self.run, valid_from)?;
            let repeats_prior = hour
                .checked_sub(1)
                .map(LeadHour)
                .and_then(|prior| self.product.canonical_lead(self.run, prior))
                == Some(valid_through);
            (!repeats_prior).then_some(LeadDetent {
                valid_from,
                valid_through,
            })
        })
    }

    pub(crate) fn detent_count(self) -> u16 {
        self.detents().fold(0_u16, |count, _detent| count + 1)
    }

    pub(crate) fn index_at_or_before(self, lead: LeadHour) -> u16 {
        self.detents()
            .take_while(|detent| detent.valid_from <= lead)
            .fold(0_u16, |count, _detent| count + 1)
            .saturating_sub(1)
    }

    pub(crate) fn index_at_or_after(self, lead: LeadHour) -> u16 {
        self.detents()
            .zip(0_u16..)
            .find_map(|(detent, index)| (detent.valid_from >= lead).then_some(index))
            .unwrap_or_else(|| self.detent_count().saturating_sub(1))
    }

    pub(crate) fn at(self, index: u16) -> Option<LeadHour> {
        self.detents()
            .nth(usize::from(index))
            .map(|detent| detent.valid_from)
    }

    pub(crate) fn ready_ceiling(self, frontier: LeadHour) -> Option<u16> {
        self.detents()
            .take_while(|detent| detent.valid_through <= frontier)
            .fold(0_u16, |count, _detent| count + 1)
            .checked_sub(1)
    }

    pub(crate) fn snap(self, lead: LeadHour, frontier: LeadHour) -> Option<LeadHour> {
        self.detents()
            .take_while(|detent| detent.valid_from <= lead)
            .filter(|detent| detent.valid_through <= frontier)
            .last()
            .map(|detent| detent.valid_from)
    }

    pub(crate) fn ready(self, lead: LeadHour, frontier: LeadHour) -> bool {
        self.detents()
            .any(|detent| detent.valid_from == lead && detent.valid_through <= frontier)
    }

    pub(crate) fn local_label(self, lead: LeadHour) -> Result<String> {
        if self
            .detents()
            .any(|detent| detent.valid_through.get() - detent.valid_from.get() >= 23)
        {
            self.run.id.valid_local_date_label(lead)
        } else {
            self.run.valid_local_label(lead)
        }
    }
}

fn aqm_day_zero(run: RunId) -> Result<i8> {
    const DAILY_BOUNDARY_UTC: i8 = 5;
    let cycle = i8::try_from(run.cycle()?)?;
    if matches!(cycle, 6 | 12) {
        Ok(DAILY_BOUNDARY_UTC - cycle)
    } else {
        bail!("AQM run must use a 06Z or 12Z cycle, found {cycle:02}Z");
    }
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

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum ForecastSystem {
    Hrrr,
    Aqm,
}

impl ForecastSystem {
    pub(crate) const ALL: [Self; 2] = [Self::Hrrr, Self::Aqm];

    pub(crate) fn cycle_at_or_before(self, timestamp: Timestamp) -> Result<RunId> {
        let hour = RunId::hourly_at_or_before(timestamp);
        Ok(match self {
            Self::Hrrr => hour,
            Self::Aqm => {
                let cycle = hour.cycle()?;
                let retreat = match cycle {
                    0..=5 => cycle.saturating_add(12),
                    6..=11 => cycle - 6,
                    _ => cycle - 12,
                };
                hour.hours_ago(retreat)
            }
        })
    }

    pub(crate) fn previous(self, run: RunId) -> Result<RunId> {
        Ok(match self {
            Self::Hrrr => run.hours_ago(1),
            Self::Aqm if run.cycle()? == 12 => run.hours_ago(6),
            Self::Aqm => run.hours_ago(18),
        })
    }

    pub(crate) fn next(self, run: RunId) -> Result<RunId> {
        Ok(match self {
            Self::Hrrr => run.hours_after(1),
            Self::Aqm if run.cycle()? == 6 => run.hours_after(6),
            Self::Aqm => run.hours_after(18),
        })
    }

    pub(crate) fn horizon(self, run: RunId) -> Result<LeadHour> {
        match self {
            Self::Hrrr => LeadHour::forge(if run.cycle()?.is_multiple_of(6) {
                48
            } else {
                18
            }),
            Self::Aqm => LeadHour::forge(72),
        }
    }

    pub(crate) fn latest_long(self, latest: RunId) -> Result<RunId> {
        match self {
            Self::Hrrr => Ok(latest.hours_ago(latest.cycle()? % 6)),
            Self::Aqm => Ok(latest),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct ForecastRun {
    pub system: ForecastSystem,
    pub id: RunId,
}

impl ForecastRun {
    pub(crate) const fn forge(system: ForecastSystem, id: RunId) -> Self {
        Self { system, id }
    }

    pub(crate) const fn hrrr(id: RunId) -> Self {
        Self::forge(ForecastSystem::Hrrr, id)
    }

    pub(crate) fn horizon(self) -> Result<LeadHour> {
        self.system.horizon(self.id)
    }

    pub(crate) fn previous(self) -> Result<Self> {
        Ok(Self::forge(self.system, self.system.previous(self.id)?))
    }

    pub(crate) fn next(self) -> Result<Self> {
        Ok(Self::forge(self.system, self.system.next(self.id)?))
    }

    pub(crate) fn rebase_lead(
        self,
        source: Self,
        source_lead: LeadHour,
        frontier: LeadHour,
    ) -> LeadHour {
        self.id.rebase_lead(source.id, source_lead, frontier)
    }

    pub(crate) fn local_label(self) -> Result<String> {
        self.id.local_label()
    }

    pub(crate) fn valid_local_label(self, lead: LeadHour) -> Result<String> {
        self.id.valid_local_label(lead)
    }
}

impl RunSelection {
    pub(crate) fn bind(self, system: ForecastSystem, latest: RunId) -> RunId {
        match self {
            Self::Latest => latest,
            Self::LatestLong => system.latest_long(latest).unwrap_or(latest),
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
            bail!("forecast cycle {epoch_second} is not aligned to an hour");
        }
        let _timestamp = Timestamp::from_second(epoch_second)
            .context("forecast cycle lies outside civil time")?;
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

    pub(crate) fn timestamp_at_offset(self, hours: i16) -> Result<Timestamp> {
        Timestamp::from_second(self.0 + i64::from(hours) * 3_600).map_err(Into::into)
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

    fn valid_local_date_label(self, lead: LeadHour) -> Result<String> {
        Ok(self
            .valid_timestamp(lead)?
            .to_zoned(TimeZone::system())
            .strftime("%a %b %e")
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
    pub const ONE: Self = Self(1);
    pub const MAX: u8 = 72;

    pub fn forge(hour: u8) -> Result<Self> {
        if hour > Self::MAX {
            bail!("forecast lead {hour} exceeds ceiling {}", Self::MAX);
        }
        Ok(Self(hour))
    }

    pub const fn get(self) -> u8 {
        self.0
    }

    pub fn saturating_next(self, horizon: Self) -> Self {
        Self(self.0.saturating_add(1).min(horizon.0))
    }

    pub fn next(self) -> Option<Self> {
        self.0
            .checked_add(1)
            .filter(|hour| *hour <= Self::MAX)
            .map(Self)
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
    run: ForecastRun,
    published: LeadHour,
}

impl RunExtent {
    pub(crate) fn forge(run: ForecastRun, published: LeadHour) -> Result<Self> {
        let horizon = run.horizon()?;
        if published > horizon {
            bail!("published lead {published} exceeds {run:?} horizon {horizon}");
        }
        Ok(Self { run, published })
    }

    pub(crate) const fn run(self) -> ForecastRun {
        self.run
    }

    pub const fn published(self) -> LeadHour {
        self.published
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct BladeKey {
    pub run: ForecastRun,
    pub lead: LeadHour,
    pub product: Product,
    ingredient: Ingredient,
}

impl BladeKey {
    pub(crate) fn forge(
        run: ForecastRun,
        lead: LeadHour,
        product: Product,
        ingredient: Ingredient,
    ) -> Option<Self> {
        (product.system() == run.system)
            .then(|| product.ingredient_law(ingredient))
            .flatten()
            .and_then(|_law| {
                let lead = product.canonical_lead(run, lead)?;
                Some(Self {
                    run,
                    lead,
                    product,
                    ingredient,
                })
            })
    }

    pub(crate) fn index_match(self, descriptor: &str) -> bool {
        self.law().is_some_and(|law| match law.acquisition {
            AcquisitionLaw::Hrrr(inventory) => inventory.matches(descriptor),
            AcquisitionLaw::Aqm(_) => false,
        })
    }

    pub(crate) fn cache_name(self) -> Option<String> {
        self.law().map(|law| {
            if law.cache_suffix.is_empty() {
                self.product.cache_name().to_owned()
            } else {
                format!("{}-{}", self.product.cache_name(), law.cache_suffix)
            }
        })
    }

    pub(crate) fn grib_law(self) -> Option<GribLaw> {
        self.law().map(|law| law.grib)
    }

    pub(crate) fn aqm_bundle(self) -> Option<AqmBundle> {
        match self.law()?.acquisition {
            AcquisitionLaw::Aqm(bundle) => Some(bundle),
            AcquisitionLaw::Hrrr(_) => None,
        }
    }

    pub(crate) fn daily_slot(self) -> Option<u8> {
        self.product.daily_slot(self.run, self.lead)
    }

    pub(crate) fn forecast_start(self) -> Result<i16> {
        let law = self
            .grib_law()
            .context("blade key escaped its product recipe")?;
        Ok(match law.time {
            GribTimeLaw::Instant => i16::from(self.lead.get()),
            GribTimeLaw::AccumulationFromRun => 0,
            GribTimeLaw::HourlyAccumulation => i16::from(self.lead.get().saturating_sub(1)),
            GribTimeLaw::DailySummary { start_shift, .. } => {
                i16::from(aqm_day_zero(self.run.id)?)
                    + i16::from(self.daily_slot().context("daily blade has no day slot")?) * 24
                    + i16::from(start_shift)
            }
        })
    }

    pub(crate) fn interval_end(self) -> Result<Timestamp> {
        let law = self
            .grib_law()
            .context("blade key escaped its product recipe")?;
        match law.time {
            GribTimeLaw::DailySummary { end_shift, .. } => {
                let slot = self.daily_slot().context("daily blade has no day slot")?;
                let end = i16::from(aqm_day_zero(self.run.id)?)
                    + i16::from(slot) * 24
                    + 23
                    + i16::from(end_shift);
                self.run.id.timestamp_at_offset(end)
            }
            GribTimeLaw::Instant
            | GribTimeLaw::AccumulationFromRun
            | GribTimeLaw::HourlyAccumulation => self.run.id.valid_timestamp(self.lead),
        }
    }

    pub(crate) const fn ingredient(self) -> Ingredient {
        self.ingredient
    }

    pub(crate) const fn is_vector_component(self) -> bool {
        matches!(self.product.shape(), FieldShape::Vector)
    }

    const fn law(self) -> Option<IngredientLaw> {
        self.product.ingredient_law(self.ingredient)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FrameKey {
    pub run: ForecastRun,
    pub valid: LeadHour,
    pub product: Product,
    baseline: Option<LeadHour>,
}

impl FrameKey {
    pub fn forge(
        run: ForecastRun,
        product: Product,
        baseline: LeadHour,
        valid: LeadHour,
    ) -> Option<Self> {
        if product.system() != run.system {
            return None;
        }
        if valid > product.horizon(run).ok()? {
            return None;
        }
        let baseline = match product.temporal_shape() {
            TemporalShape::Cumulative if baseline < valid => Some(baseline),
            TemporalShape::Cumulative => return None,
            TemporalShape::Instant | TemporalShape::Interval => None,
        };
        Some(Self {
            run,
            valid,
            product,
            baseline,
        })
    }

    pub const fn baseline(self) -> Option<LeadHour> {
        self.baseline
    }

    pub(crate) fn blade_at(self, lead: LeadHour, ingredient: Ingredient) -> Option<BladeKey> {
        BladeKey::forge(self.run, lead, self.product, ingredient)
    }

    pub fn with_valid(self, valid: LeadHour) -> Option<Self> {
        Self::forge(
            self.run,
            self.product,
            self.baseline.unwrap_or(LeadHour::ZERO),
            valid,
        )
    }

    pub(crate) fn field_identity(self) -> Self {
        Self {
            valid: self
                .product
                .canonical_lead(self.run, self.valid)
                .unwrap_or(self.valid),
            ..self
        }
    }
}

impl fmt::Display for FrameKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(baseline) = self.baseline() {
            write!(formatter, "{baseline}–{}", self.valid)
        } else {
            self.valid.fmt(formatter)
        }
    }
}

/// The spherical Lambert conformal law carried by each forecast GRIB message.
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

    pub fn lon_lat_at_grid(self, i: f64, j: f64) -> [f64; 2] {
        let x = i.mul_add(self.spacing[0], self.first_xy[0]);
        let y = j.mul_add(self.spacing[1], self.first_xy[1]);
        let poleward = self.origin_rho - y;
        let rho = x.hypot(poleward);
        let latitude = 2.0 * (self.radius_factor / rho).powf(1.0 / self.cone).atan()
            - std::f64::consts::FRAC_PI_2;
        let longitude = (self.central_lon + x.atan2(poleward) / self.cone).to_degrees();
        [
            (longitude + 180.0).rem_euclid(360.0) - 180.0,
            latitude.to_degrees(),
        ]
    }

    fn earth_vector_at_grid(self, i: u32, j: u32, [x, y]: [f32; 2]) -> [f32; 2] {
        let grid_x = f64::from(i).mul_add(self.spacing[0], self.first_xy[0]);
        let grid_y = f64::from(j).mul_add(self.spacing[1], self.first_xy[1]);
        let convergence = grid_x.atan2(self.origin_rho - grid_y);
        let (sin, cos) = convergence.sin_cos();
        [
            f64::from(x).mul_add(cos, f64::from(y) * sin) as f32,
            (-f64::from(x)).mul_add(sin, f64::from(y) * cos) as f32,
        ]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum VectorFrame {
    Earth,
    Grid,
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
    /// About two kilometres across 1,920 points at the default 38.5° latitude.
    pub const MAX_ZOOM: f64 = 16.85;

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
    vector_frame: Option<VectorFrame>,
    vector: Option<VectorGrid>,
}

#[derive(Clone, Debug)]
struct VectorGrid {
    eastward: Arc<[f32]>,
    northward: Arc<[f32]>,
}

impl FieldGrid {
    pub fn forge(
        values: Vec<f32>,
        width: usize,
        height: usize,
        projection: LambertGrid,
    ) -> Result<Self> {
        Self::forge_blade(values, width, height, projection, None)
    }

    pub(crate) fn forge_blade(
        values: Vec<f32>,
        width: usize,
        height: usize,
        projection: LambertGrid,
        vector_frame: Option<VectorFrame>,
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
            vector_frame,
            vector: None,
        })
    }

    pub fn forge_vector(eastward: &Self, northward: &Self) -> Result<Self> {
        if eastward.width != northward.width
            || eastward.height != northward.height
            || eastward.projection != northward.projection
        {
            bail!("vector components do not share one grid law");
        }
        if eastward.vector.is_some() || northward.vector.is_some() {
            bail!("vector components must be scalar GRIB blades");
        }
        let Some(frame) = eastward.vector_frame else {
            bail!("eastward blade has no vector coordinate frame");
        };
        if northward.vector_frame != Some(frame) {
            bail!("vector components do not share one coordinate frame");
        }
        let mut east = Vec::with_capacity(eastward.values.len());
        let mut north = Vec::with_capacity(northward.values.len());
        let mut values = Vec::with_capacity(eastward.values.len());
        let width = usize::try_from(eastward.width)?;
        for (slot, (&x, &y)) in eastward
            .values
            .iter()
            .zip(northward.values.iter())
            .enumerate()
        {
            let vector = match frame {
                VectorFrame::Earth => [x, y],
                VectorFrame::Grid => eastward.projection.earth_vector_at_grid(
                    u32::try_from(slot % width)?,
                    u32::try_from(slot / width)?,
                    [x, y],
                ),
            };
            east.push(vector[0]);
            north.push(vector[1]);
            values.push(vector[0].hypot(vector[1]));
        }
        Ok(Self {
            values: values.into(),
            width: eastward.width,
            height: eastward.height,
            projection: eastward.projection,
            vector_frame: None,
            vector: Some(VectorGrid {
                eastward: east.into(),
                northward: north.into(),
            }),
        })
    }

    pub fn at(&self, i: u32, j: u32) -> Option<f32> {
        if i >= self.width || j >= self.height {
            return None;
        }
        self.values.get((j * self.width + i) as usize).copied()
    }

    pub fn vector_at(&self, i: u32, j: u32) -> Option<[f32; 2]> {
        if i >= self.width || j >= self.height {
            return None;
        }
        let vector = self.vector.as_ref()?;
        let slot = (j * self.width + i) as usize;
        Some([*vector.eastward.get(slot)?, *vector.northward.get(slot)?])
    }

    pub fn increment_since(&self, baseline: &Self) -> Result<Self> {
        if self.vector.is_some() || baseline.vector.is_some() {
            bail!("vector fields cannot form cumulative increments");
        }
        if self.width != baseline.width
            || self.height != baseline.height
            || self.projection != baseline.projection
        {
            bail!("cumulative endpoints do not share one grid law");
        }
        let values = self
            .values
            .iter()
            .zip(baseline.values.iter())
            .map(|(&total, &base)| {
                let increment = total - base;
                if increment.is_finite() {
                    increment.max(0.0)
                } else {
                    increment
                }
            })
            .collect();
        Self::forge(
            values,
            self.width as usize,
            self.height as usize,
            self.projection,
        )
    }

    pub(crate) fn fuse3(
        first: &Self,
        second: &Self,
        third: &Self,
        mut fuse: impl FnMut(f32, f32, f32) -> f32,
    ) -> Result<Self> {
        if [second, third].iter().any(|field| {
            field.width != first.width
                || field.height != first.height
                || field.projection != first.projection
        }) {
            bail!("derived field ingredients do not share one grid law");
        }
        if [first, second, third]
            .iter()
            .any(|field| field.vector.is_some())
        {
            bail!("derived scalar fields require scalar ingredients");
        }
        let values = first
            .values
            .iter()
            .zip(second.values.iter())
            .zip(third.values.iter())
            .map(|((&a, &b), &c)| fuse(a, b, c))
            .collect();
        Self::forge(
            values,
            first.width as usize,
            first.height as usize,
            first.projection,
        )
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
    fn cumulative_frame_identity_requires_a_nonempty_span() -> Result<()> {
        let run = ForecastRun::hrrr(RunId::forge(1_785_272_400)?);
        let base = LeadHour::forge(3)?;
        let valid = LeadHour::forge(8)?;
        let frame = FrameKey::forge(run, Product::QpfRun, base, valid).context("QPF span")?;
        assert_eq!(frame.baseline(), Some(base));
        assert_eq!(
            frame
                .blade_at(valid, Ingredient::Scalar)
                .context("QPF scalar blade")?
                .lead,
            valid
        );
        assert_eq!(
            frame
                .blade_at(base, Ingredient::Scalar)
                .map(|blade| blade.lead),
            Some(base)
        );
        assert_eq!(frame.to_string(), "F03–F08");
        assert!(FrameKey::forge(run, Product::QpfRun, valid, valid).is_none());

        let hourly =
            FrameKey::forge(run, Product::QpfHour, base, valid).context("hourly QPF frame")?;
        assert_eq!(hourly.baseline(), None);
        assert_eq!(hourly.to_string(), "F08");
        Ok(())
    }

    #[test]
    fn cumulative_increment_obeys_grid_identity_and_nonnegativity() -> Result<()> {
        let projection = LambertGrid {
            cone: 1.0,
            radius_factor: 2.0,
            origin_rho: 3.0,
            central_lon: 4.0,
            first_xy: [5.0, 6.0],
            spacing: [7.0, 8.0],
        };
        let crown = FieldGrid::forge(vec![1.0, 5.0, f32::NAN], 3, 1, projection)?;
        let root = FieldGrid::forge(vec![2.0, 3.0, 0.0], 3, 1, projection)?;
        let increment = crown.increment_since(&root)?;
        assert_eq!(&increment.values[..2], &[0.0, 2.0]);
        assert!(increment.values[2].is_nan());

        let alien = FieldGrid::forge(vec![0.0], 1, 1, projection)?;
        assert!(crown.increment_since(&alien).is_err());
        Ok(())
    }

    #[test]
    fn vector_recipe_resolves_its_coordinate_frame_before_scalar_projection() -> Result<()> {
        let projection = LambertGrid {
            cone: 1.0,
            radius_factor: 1.0,
            origin_rho: 1.0,
            central_lon: 0.0,
            first_xy: [0.0; 2],
            spacing: [1.0; 2],
        };
        let eastward =
            FieldGrid::forge_blade(vec![3.0, 0.0], 2, 1, projection, Some(VectorFrame::Earth))?;
        let northward =
            FieldGrid::forge_blade(vec![4.0, -2.0], 2, 1, projection, Some(VectorFrame::Earth))?;
        let wind = FieldGrid::forge_vector(&eastward, &northward)?;
        assert_eq!(&wind.values[..], &[5.0, 2.0]);
        assert_eq!(wind.vector_at(0, 0), Some([3.0, 4.0]));
        assert_eq!(wind.vector_at(1, 0), Some([0.0, -2.0]));

        let converged = LambertGrid {
            first_xy: [1.0, 0.0],
            ..projection
        };
        let x = FieldGrid::forge_blade(
            vec![std::f32::consts::SQRT_2],
            1,
            1,
            converged,
            Some(VectorFrame::Grid),
        )?;
        let y = FieldGrid::forge_blade(vec![0.0], 1, 1, converged, Some(VectorFrame::Grid))?;
        let rotated = FieldGrid::forge_vector(&x, &y)?;
        let [east, north] = rotated.vector_at(0, 0).context("resolved vector")?;
        assert!((east - 1.0).abs() < 1.0e-6);
        assert!((north + 1.0).abs() < 1.0e-6);
        Ok(())
    }

    #[test]
    fn extended_cycles_own_the_long_horizon() -> Result<()> {
        let run = RunId::hourly_at_or_before(Timestamp::from_second(1_752_926_400)?);
        assert_eq!(run.cycle()?, 12);
        assert_eq!(ForecastSystem::Hrrr.horizon(run)?.get(), 48);
        assert_eq!(ForecastSystem::Hrrr.horizon(run.hours_ago(1))?.get(), 18);
        assert_eq!(ForecastSystem::Hrrr.latest_long(run)?, run);
        assert_eq!(ForecastSystem::Hrrr.latest_long(run.hours_after(5))?, run);
        assert_eq!(
            RunSelection::LatestLong.bind(ForecastSystem::Hrrr, run.hours_after(5)),
            run
        );
        assert_eq!(
            RunSelection::Fixed(run.hours_ago(1)).bind(ForecastSystem::Hrrr, run),
            run.hours_ago(1)
        );

        let aqm = ForecastRun::forge(ForecastSystem::Aqm, run);
        assert_eq!(Product::AirQuality.horizon(aqm)?.get(), 64);
        assert_eq!(Product::AirQuality.horizon(aqm.previous()?)?.get(), 70);
        let axis = Product::AirQuality.lead_axis(aqm)?;
        assert_eq!(
            (0..axis.detent_count())
                .filter_map(|index| axis.at(index).map(LeadHour::get))
                .collect::<Vec<_>>(),
            [0, 17, 41]
        );
        let prior_axis = Product::AirQuality.lead_axis(aqm.previous()?)?;
        assert_eq!(
            (0..prior_axis.detent_count())
                .filter_map(|index| prior_axis.at(index).map(LeadHour::get))
                .collect::<Vec<_>>(),
            [0, 23, 47]
        );
        assert_eq!(aqm.previous()?.id, run.hours_ago(6));
        assert_eq!(aqm.previous()?.previous()?.id, run.hours_ago(24));
        assert_eq!(
            FrameKey::forge(aqm, Product::AirQuality, LeadHour::ZERO, LeadHour::ZERO)
                .context("first AQI hour")?
                .field_identity(),
            FrameKey::forge(
                aqm,
                Product::AirQuality,
                LeadHour::ZERO,
                LeadHour::forge(16)?,
            )
            .context("last hour in first AQI period")?
            .field_identity(),
        );
        assert_ne!(
            FrameKey::forge(aqm, Product::AirQuality, LeadHour::ZERO, LeadHour::ZERO)
                .context("first AQI hour")?
                .field_identity(),
            FrameKey::forge(
                aqm,
                Product::AirQuality,
                LeadHour::ZERO,
                LeadHour::forge(17)?,
            )
            .context("first hour in second AQI period")?
            .field_identity(),
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
        let run = ForecastRun::hrrr(
            RunId::hourly_at_or_before(Timestamp::from_second(1_752_926_400)?).hours_ago(1),
        );
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
