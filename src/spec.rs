use crate::model::{FrameKey, Product};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Unit {
    Inch,
    MicrogramPerCubicMetre,
    Fahrenheit,
    Percent,
    MilesPerHour,
    Hectopascal,
    AirQualityIndex,
}

impl Unit {
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::Inch => "in",
            Self::MicrogramPerCubicMetre => "µg/m³",
            Self::Fahrenheit => "°F",
            Self::Percent => "%",
            Self::MilesPerHour => "mph",
            Self::Hectopascal => "hPa",
            Self::AirQualityIndex => "AQI",
        }
    }

    pub fn convert(self, raw: f32) -> f32 {
        let [gain, bias] = self.affine();
        raw.mul_add(gain, bias)
    }

    pub const fn affine(self) -> [f32; 2] {
        match self {
            Self::Inch => [1.0 / 25.4, 0.0],
            Self::MicrogramPerCubicMetre => [1.0e9, 0.0],
            Self::Fahrenheit => [1.8, -459.67],
            Self::Percent => [1.0, 0.0],
            Self::MilesPerHour => [2.236_936_3, 0.0],
            Self::Hectopascal => [0.01, 0.0],
            Self::AirQualityIndex => [1.0, 0.0],
        }
    }

    pub fn format(self, value: f32) -> String {
        match self {
            Self::Inch => format!("{value:.2} {}", self.symbol()),
            Self::MicrogramPerCubicMetre => format!("{value:.1} {}", self.symbol()),
            Self::Fahrenheit => format!("{value:.1}{}", self.symbol()),
            Self::Percent => format!("{value:.0}{}", self.symbol()),
            Self::MilesPerHour => format!("{value:.1} {}", self.symbol()),
            Self::Hectopascal => format!("{value:.0} {}", self.symbol()),
            Self::AirQualityIndex => format!("{value:.0} {}", self.symbol()),
        }
    }

    pub fn format_ceiling(self, value: f32) -> String {
        match self {
            Self::Percent | Self::Fahrenheit => format!("{value:.0}{}", self.symbol()),
            Self::Inch
            | Self::MicrogramPerCubicMetre
            | Self::MilesPerHour
            | Self::Hectopascal
            | Self::AirQualityIndex => {
                format!("{value:.0} {}", self.symbol())
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bin {
    pub ceiling: f32,
    pub srgb: [u8; 4],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Contour {
    pub width_points: f32,
    pub srgb: [u8; 4],
}

impl Bin {
    const fn new(ceiling: f32, srgb: [u8; 4]) -> Self {
        Self { ceiling, srgb }
    }
}

#[derive(Clone, Debug)]
pub struct Scale {
    pub unit: Unit,
    pub bins: Arc<[Bin]>,
    pub contour: Contour,
}

impl Scale {
    fn forge(unit: Unit, bins: impl Into<Arc<[Bin]>>, contour: Contour) -> Self {
        Self {
            unit,
            bins: bins.into(),
            contour,
        }
    }

    pub fn display(&self, raw: f32) -> String {
        self.unit.format(self.unit.convert(raw))
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RangeRegime {
    #[default]
    Fine,
    Broad,
}

impl RangeRegime {
    fn reckon(self, visible_peak: Option<f32>, gate: RangeGate) -> Self {
        let Some(visible_peak) = visible_peak else {
            return self;
        };
        match self {
            Self::Fine if visible_peak > gate.broad_onset => Self::Broad,
            Self::Broad if visible_peak < gate.fine_return => Self::Fine,
            _ => self,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct RangeGate {
    broad_onset: f32,
    fine_return: f32,
}

impl RangeGate {
    const fn forge(broad_onset: f32, fine_return: f32) -> Self {
        assert!(fine_return < broad_onset);
        Self {
            broad_onset,
            fine_return,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TemperatureSeason {
    Summer,
    #[default]
    Winter,
}

impl TemperatureSeason {
    pub fn at(key: FrameKey) -> Self {
        key.run
            .id
            .valid_month_utc(key.valid)
            .map_or(Self::Winter, Self::for_month)
    }

    const fn for_month(month: i8) -> Self {
        if month >= 5 && month <= 9 {
            Self::Summer
        } else {
            Self::Winter
        }
    }
}

#[derive(Clone, Debug)]
enum ScaleFamily {
    Static(Scale),
    Adaptive {
        fine: Scale,
        broad: Scale,
        gate: RangeGate,
    },
    Seasonal {
        summer: Scale,
        winter: Scale,
    },
}

impl ScaleFamily {
    const fn adaptive(&self) -> bool {
        matches!(self, Self::Adaptive { .. })
    }

    fn reckon(&self, settled: RangeRegime, raw_peak: Option<f32>) -> RangeRegime {
        let Self::Adaptive { fine, gate, .. } = self else {
            return settled;
        };
        settled.reckon(raw_peak.map(|raw| fine.unit.convert(raw)), *gate)
    }

    fn resolve(&self, regime: RangeRegime, temperature: TemperatureSeason) -> &Scale {
        match (self, regime, temperature) {
            (Self::Static(scale), _, _) => scale,
            (Self::Adaptive { fine, .. }, RangeRegime::Fine, _) => fine,
            (Self::Adaptive { broad, .. }, RangeRegime::Broad, _) => broad,
            (Self::Seasonal { summer, .. }, _, TemperatureSeason::Summer) => summer,
            (Self::Seasonal { winter, .. }, _, TemperatureSeason::Winter) => winter,
        }
    }
}

macro_rules! scale_arsenal {
    ($($product:ident => $field:ident = $family:expr),+ $(,)?) => {
        #[derive(Clone, Debug)]
        pub struct ScaleAtlas {
            $($field: ScaleFamily),+
        }

        impl Default for ScaleAtlas {
            fn default() -> Self {
                Self {
                    $($field: $family),+
                }
            }
        }

        impl ScaleAtlas {
            fn family(&self, product: Product) -> &ScaleFamily {
                match product {
                    $(Product::$product => &self.$field),+
                }
            }

            pub fn adaptive(&self, product: Product) -> bool {
                self.family(product).adaptive()
            }

            pub fn reckon(
                &self,
                product: Product,
                settled: RangeRegime,
                raw_peak: Option<f32>,
            ) -> RangeRegime {
                self.family(product).reckon(settled, raw_peak)
            }

            pub fn get(
                &self,
                product: Product,
                regime: RangeRegime,
                temperature: TemperatureSeason,
            ) -> &Scale {
                self.family(product).resolve(regime, temperature)
            }
        }
    };
}

scale_arsenal! {
    QpfRun => qpf_run = ScaleFamily::Static(Scale::forge(
        Unit::Inch,
        QPF,
        PRECIPITATION_CONTOUR,
    )),
    QpfHour => qpf_hour = ScaleFamily::Static(Scale::forge(
        Unit::Inch,
        &QPF[..QPF_HOURLY_BINS],
        PRECIPITATION_CONTOUR,
    )),
    Smoke => smoke = ScaleFamily::Adaptive {
        fine: Scale::forge(Unit::MicrogramPerCubicMetre, FINE_SMOKE, SMOKE_CONTOUR),
        broad: Scale::forge(Unit::MicrogramPerCubicMetre, BROAD_SMOKE, SMOKE_CONTOUR),
        gate: RangeGate::forge(40.0, 20.0),
    },
    Temperature => temperature = ScaleFamily::Seasonal {
        summer: Scale::forge(
            Unit::Fahrenheit,
            gradient_bins(40, 120, TEMPERATURE_ALPHA, &SUMMER_TEMPERATURE),
            TEMPERATURE_CONTOUR,
        ),
        winter: Scale::forge(
            Unit::Fahrenheit,
            gradient_bins(-50, 80, TEMPERATURE_ALPHA, &WINTER_TEMPERATURE),
            TEMPERATURE_CONTOUR,
        ),
    },
    DewPoint => dew_point = ScaleFamily::Static(Scale::forge(
        Unit::Fahrenheit,
        gradient_bins(-40, 90, DEW_POINT_ALPHA, &DEW_POINT),
        DEW_POINT_CONTOUR,
    )),
    CloudCover => cloud_cover = ScaleFamily::Static(Scale::forge(
        Unit::Percent,
        CLOUD_COVER,
        CLOUD_CONTOUR,
    )),
    Pressure => pressure = ScaleFamily::Static(Scale::forge(
        Unit::Hectopascal,
        gradient_bins(880, 1080, PRESSURE_ALPHA, &PRESSURE),
        PRESSURE_CONTOUR,
    )),
    Wind => wind = ScaleFamily::Static(Scale::forge(
        Unit::MilesPerHour,
        WIND,
        WIND_CONTOUR,
    )),
    AirQuality => air_quality = ScaleFamily::Adaptive {
        fine: Scale::forge(
            Unit::AirQualityIndex,
            atmospheric_gradient_bins(0, 100, 5, &AIR_QUALITY_RAMP),
            AIR_QUALITY_CONTOUR,
        ),
        broad: Scale::forge(Unit::AirQualityIndex, BROAD_AIR_QUALITY, AIR_QUALITY_CONTOUR),
        gate: RangeGate::forge(100.0, 75.0),
    },
}

const VOID: [u8; 4] = [10, 10, 8, 0];
const PRECIPITATION_CONTOUR: Contour = Contour {
    width_points: 0.28,
    srgb: [35, 31, 26, 52],
};
const SMOKE_CONTOUR: Contour = Contour {
    width_points: 0.40,
    srgb: [35, 31, 26, 58],
};
const TEMPERATURE_CONTOUR: Contour = Contour {
    width_points: 0.18,
    srgb: [35, 31, 26, 34],
};
const DEW_POINT_CONTOUR: Contour = Contour {
    width_points: 0.18,
    srgb: [35, 31, 26, 38],
};
const CLOUD_CONTOUR: Contour = Contour {
    width_points: 0.18,
    srgb: [35, 31, 26, 38],
};
const PRESSURE_CONTOUR: Contour = Contour {
    width_points: 0.22,
    srgb: [35, 31, 26, 58],
};
const WIND_CONTOUR: Contour = Contour {
    width_points: 0.22,
    srgb: [35, 31, 26, 46],
};
const AIR_QUALITY_CONTOUR: Contour = Contour {
    width_points: 0.30,
    srgb: [35, 31, 26, 54],
};
const QPF_HOURLY_BINS: usize = 15;
const QPF: [Bin; 18] = [
    Bin::new(0.00, VOID),
    Bin::new(0.01, [104, 151, 143, 70]),
    Bin::new(0.02, [91, 160, 146, 82]),
    Bin::new(0.05, [76, 170, 147, 100]),
    Bin::new(0.10, [57, 177, 144, 122]),
    Bin::new(0.15, [42, 174, 158, 138]),
    Bin::new(0.25, [35, 164, 177, 158]),
    Bin::new(0.35, [39, 148, 193, 176]),
    Bin::new(0.50, [48, 128, 199, 192]),
    Bin::new(0.75, [63, 105, 191, 205]),
    Bin::new(1.00, [82, 84, 174, 215]),
    Bin::new(1.50, [111, 68, 158, 223]),
    Bin::new(2.00, [142, 62, 140, 229]),
    Bin::new(3.00, [175, 66, 114, 234]),
    Bin::new(4.00, [202, 80, 86, 238]),
    Bin::new(6.00, [222, 112, 67, 241]),
    Bin::new(8.00, [237, 156, 78, 244]),
    Bin::new(10.0, [247, 207, 126, 247]),
];
const ATMOSPHERIC_RAMP: [[u8; 4]; 13] = [
    VOID,
    [132, 161, 179, 58],
    [112, 151, 176, 69],
    [92, 140, 169, 81],
    [73, 127, 158, 95],
    [76, 132, 144, 109],
    [142, 145, 102, 126],
    [195, 155, 78, 148],
    [219, 112, 58, 176],
    [194, 67, 56, 201],
    [147, 52, 85, 221],
    [91, 48, 103, 236],
    [39, 31, 42, 246],
];
const FINE_ATMOSPHERIC_TONES: [usize; 10] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
const BROAD_ATMOSPHERIC_TONES: [usize; 9] = [0, 4, 6, 7, 8, 9, 10, 11, 12];
const FINE_SMOKE: [Bin; 10] = atmospheric_bins(
    [0.0, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 40.0],
    FINE_ATMOSPHERIC_TONES,
);
const BROAD_SMOKE: [Bin; 9] = atmospheric_bins(
    [0.0, 1.0, 5.0, 10.0, 20.0, 40.0, 80.0, 160.0, 320.0],
    BROAD_ATMOSPHERIC_TONES,
);
const BROAD_AIR_QUALITY: [Bin; 9] = atmospheric_bins(
    [0.0, 25.0, 50.0, 75.0, 100.0, 150.0, 200.0, 300.0, 500.0],
    BROAD_ATMOSPHERIC_TONES,
);

const fn atmospheric_bins<const N: usize>(ceilings: [f32; N], tones: [usize; N]) -> [Bin; N] {
    let mut bins = [Bin::new(0.0, VOID); N];
    let mut slot = 0;
    while slot < N {
        bins[slot] = Bin::new(ceilings[slot], ATMOSPHERIC_RAMP[tones[slot]]);
        slot += 1;
    }
    bins
}

#[derive(Clone, Copy)]
struct RampAnchor {
    value: f32,
    tone: f32,
}

impl RampAnchor {
    const fn new(value: f32, tone: u8) -> Self {
        Self {
            value,
            tone: tone as f32,
        }
    }
}

const AIR_QUALITY_RAMP: [RampAnchor; 5] = [
    RampAnchor::new(0.0, 0),
    RampAnchor::new(25.0, 4),
    RampAnchor::new(50.0, 6),
    RampAnchor::new(75.0, 7),
    RampAnchor::new(100.0, 8),
];

fn atmospheric_gradient_bins(
    minimum: i16,
    maximum: i16,
    step: usize,
    anchors: &[RampAnchor],
) -> Arc<[Bin]> {
    assert!(!anchors.is_empty(), "an atmospheric ramp needs anchors");
    (minimum..=maximum)
        .step_by(step)
        .map(|value| {
            let ceiling = f32::from(value);
            Bin::new(ceiling, atmospheric_gradient_color(ceiling, anchors))
        })
        .collect()
}

fn atmospheric_gradient_color(value: f32, anchors: &[RampAnchor]) -> [u8; 4] {
    let mut lo = anchors[0];
    for &hi in &anchors[1..] {
        if value <= hi.value {
            let phase = ((value - lo.value) / (hi.value - lo.value)).clamp(0.0, 1.0);
            return atmospheric_tone(lo.tone + (hi.tone - lo.tone) * phase);
        }
        lo = hi;
    }
    atmospheric_tone(lo.tone)
}

fn atmospheric_tone(tone: f32) -> [u8; 4] {
    let lo = tone.floor().max(0.0) as usize;
    let hi = tone.ceil().min((ATMOSPHERIC_RAMP.len() - 1) as f32) as usize;
    let phase = tone.fract();
    std::array::from_fn(|channel| {
        (f32::from(ATMOSPHERIC_RAMP[lo][channel])
            + (f32::from(ATMOSPHERIC_RAMP[hi][channel]) - f32::from(ATMOSPHERIC_RAMP[lo][channel]))
                * phase)
            .round() as u8
    })
}
const CLOUD_COVER: [Bin; 11] = [
    Bin::new(0.0, VOID),
    Bin::new(10.0, [91, 122, 143, 34]),
    Bin::new(20.0, [101, 129, 149, 48]),
    Bin::new(30.0, [112, 136, 155, 63]),
    Bin::new(40.0, [124, 143, 161, 79]),
    Bin::new(50.0, [138, 151, 168, 96]),
    Bin::new(60.0, [154, 160, 176, 115]),
    Bin::new(70.0, [173, 173, 185, 136]),
    Bin::new(80.0, [195, 190, 194, 158]),
    Bin::new(90.0, [219, 208, 202, 182]),
    Bin::new(100.0, [242, 231, 218, 206]),
];
const WIND: [Bin; 11] = [
    Bin::new(0.0, VOID),
    Bin::new(5.0, [122, 157, 174, 42]),
    Bin::new(10.0, [92, 145, 164, 62]),
    Bin::new(15.0, [72, 137, 145, 84]),
    Bin::new(20.0, [93, 147, 118, 108]),
    Bin::new(25.0, [153, 158, 91, 132]),
    Bin::new(30.0, [198, 153, 75, 157]),
    Bin::new(40.0, [215, 112, 67, 182]),
    Bin::new(50.0, [188, 69, 67, 204]),
    Bin::new(60.0, [137, 55, 86, 222]),
    Bin::new(80.0, [69, 43, 74, 238]),
];
const TEMPERATURE_ALPHA: u8 = 210;
const DEW_POINT_ALPHA: u8 = 205;
const PRESSURE_ALPHA: u8 = 152;

#[derive(Clone, Copy)]
struct ColorStop {
    value: f32,
    srgb: [u8; 3],
}

impl ColorStop {
    const fn new(value: f32, srgb: [u8; 3]) -> Self {
        Self { value, srgb }
    }
}

const SUMMER_TEMPERATURE: [ColorStop; 12] = [
    ColorStop::new(40.0, [55, 70, 133]),
    ColorStop::new(50.0, [52, 121, 170]),
    ColorStop::new(60.0, [58, 160, 155]),
    ColorStop::new(68.0, [98, 180, 120]),
    ColorStop::new(74.0, [162, 191, 91]),
    ColorStop::new(80.0, [217, 190, 83]),
    ColorStop::new(86.0, [228, 157, 72]),
    ColorStop::new(92.0, [220, 111, 66]),
    ColorStop::new(98.0, [196, 67, 75]),
    ColorStop::new(104.0, [158, 51, 98]),
    ColorStop::new(112.0, [105, 45, 104]),
    ColorStop::new(120.0, [55, 35, 65]),
];
const WINTER_TEMPERATURE: [ColorStop; 15] = [
    ColorStop::new(-50.0, [45, 28, 76]),
    ColorStop::new(-40.0, [59, 38, 108]),
    ColorStop::new(-30.0, [62, 58, 145]),
    ColorStop::new(-20.0, [58, 84, 178]),
    ColorStop::new(-10.0, [52, 115, 195]),
    ColorStop::new(0.0, [50, 145, 196]),
    ColorStop::new(10.0, [57, 169, 185]),
    ColorStop::new(20.0, [83, 186, 163]),
    ColorStop::new(30.0, [130, 195, 135]),
    ColorStop::new(32.0, [161, 201, 155]),
    ColorStop::new(40.0, [191, 196, 107]),
    ColorStop::new(50.0, [216, 177, 79]),
    ColorStop::new(60.0, [224, 142, 70]),
    ColorStop::new(70.0, [211, 95, 68]),
    ColorStop::new(80.0, [171, 55, 89]),
];

const DEW_POINT: [ColorStop; 13] = [
    ColorStop::new(-40.0, [104, 77, 67]),
    ColorStop::new(-20.0, [126, 94, 72]),
    ColorStop::new(0.0, [151, 119, 78]),
    ColorStop::new(20.0, [166, 146, 88]),
    ColorStop::new(32.0, [151, 164, 98]),
    ColorStop::new(40.0, [116, 169, 106]),
    ColorStop::new(50.0, [76, 165, 125]),
    ColorStop::new(55.0, [57, 156, 143]),
    ColorStop::new(60.0, [56, 143, 163]),
    ColorStop::new(65.0, [67, 125, 174]),
    ColorStop::new(70.0, [88, 100, 166]),
    ColorStop::new(75.0, [108, 72, 143]),
    ColorStop::new(90.0, [64, 39, 82]),
];

const PRESSURE: [ColorStop; 9] = [
    ColorStop::new(880.0, [53, 58, 106]),
    ColorStop::new(940.0, [57, 91, 137]),
    ColorStop::new(980.0, [76, 126, 151]),
    ColorStop::new(1000.0, [117, 150, 148]),
    ColorStop::new(1012.0, [164, 166, 143]),
    ColorStop::new(1020.0, [188, 164, 116]),
    ColorStop::new(1040.0, [191, 124, 82]),
    ColorStop::new(1060.0, [154, 76, 76]),
    ColorStop::new(1080.0, [91, 48, 71]),
];

fn gradient_bins(minimum: i16, maximum: i16, alpha: u8, stops: &[ColorStop]) -> Arc<[Bin]> {
    assert!(!stops.is_empty(), "a field ramp needs color stops");
    (minimum..=maximum)
        .step_by(2)
        .map(|value| {
            let ceiling = f32::from(value);
            Bin::new(ceiling, gradient_color(ceiling, alpha, stops))
        })
        .collect()
}

fn gradient_color(value: f32, alpha: u8, stops: &[ColorStop]) -> [u8; 4] {
    let mut lo = stops[0];
    for &hi in &stops[1..] {
        if value <= hi.value {
            let t = ((value - lo.value) / (hi.value - lo.value)).clamp(0.0, 1.0);
            let channel = |slot: usize| {
                (f32::from(lo.srgb[slot])
                    + (f32::from(hi.srgb[slot]) - f32::from(lo.srgb[slot])) * t)
                    .round() as u8
            };
            return [channel(0), channel(1), channel(2), alpha];
        }
        lo = hi;
    }
    [lo.srgb[0], lo.srgb[1], lo.srgb[2], alpha]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_preserve_physical_reference_points() {
        assert!((Unit::Inch.convert(25.4) - 1.0).abs() < 1e-6);
        assert!((Unit::Fahrenheit.convert(273.15) - 32.0).abs() < 1e-4);
        assert_eq!(Unit::MicrogramPerCubicMetre.convert(1e-9), 1.0);
        assert_eq!(Unit::Hectopascal.convert(101_325.0), 1013.25);
    }

    #[test]
    fn defaults_are_strictly_ordered() {
        let atlas = ScaleAtlas::default();
        for product in Product::ALL {
            for regime in [RangeRegime::Fine, RangeRegime::Broad] {
                for season in [TemperatureSeason::Summer, TemperatureSeason::Winter] {
                    let scale = atlas.get(product, regime, season);
                    assert!(
                        scale
                            .bins
                            .windows(2)
                            .all(|pair| pair[0].ceiling < pair[1].ceiling)
                    );
                }
            }
        }
    }

    #[test]
    fn adaptive_ranges_preserve_one_color_law() {
        let atlas = ScaleAtlas::default();
        let season = TemperatureSeason::Summer;
        for (product, shared, onset, retreat) in [
            (Product::Smoke, [0.0, 5.0, 10.0, 20.0, 40.0], 40.0, 20.0),
            (
                Product::AirQuality,
                [0.0, 25.0, 50.0, 75.0, 100.0],
                100.0,
                75.0,
            ),
        ] {
            let fine = atlas.get(product, RangeRegime::Fine, season);
            let broad = atlas.get(product, RangeRegime::Broad, season);
            for value in shared {
                let color = |scale: &Scale| {
                    scale
                        .bins
                        .iter()
                        .find(|bin| bin.ceiling == value)
                        .map(|bin| bin.srgb)
                };
                assert_eq!(color(fine), color(broad));
            }
            let [gain, bias] = fine.unit.affine();
            let raw = |value: f32| (value - bias) / gain;
            assert_eq!(
                atlas.reckon(product, RangeRegime::Fine, Some(raw(onset))),
                RangeRegime::Fine
            );
            assert_eq!(
                atlas.reckon(product, RangeRegime::Fine, Some(raw(onset + 0.1))),
                RangeRegime::Broad
            );
            assert_eq!(
                atlas.reckon(product, RangeRegime::Broad, Some(raw(retreat))),
                RangeRegime::Broad
            );
            assert_eq!(
                atlas.reckon(product, RangeRegime::Broad, Some(raw(retreat - 0.1))),
                RangeRegime::Fine
            );
        }

        let colors = |product, regime| {
            atlas
                .get(product, regime, season)
                .bins
                .iter()
                .map(|bin| bin.srgb)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            colors(Product::Smoke, RangeRegime::Broad),
            colors(Product::AirQuality, RangeRegime::Broad)
        );
        let fine_air = atlas.get(Product::AirQuality, RangeRegime::Fine, season);
        assert_eq!(
            fine_air
                .bins
                .iter()
                .map(|bin| bin.ceiling)
                .collect::<Vec<_>>(),
            (0..=100)
                .step_by(5)
                .map(|value| value as f32)
                .collect::<Vec<_>>()
        );
        assert!(
            fine_air
                .bins
                .windows(2)
                .all(|pair| pair[0].srgb != pair[1].srgb)
        );
    }

    #[test]
    fn qpf_scales_share_one_value_color_language() {
        let atlas = ScaleAtlas::default();
        let run = atlas.get(
            Product::QpfRun,
            RangeRegime::Fine,
            TemperatureSeason::Summer,
        );
        let hour = atlas.get(
            Product::QpfHour,
            RangeRegime::Fine,
            TemperatureSeason::Summer,
        );
        assert_eq!(run.bins.len(), 18);
        assert_eq!(hour.bins.len(), QPF_HOURLY_BINS);
        assert_eq!(&run.bins[..hour.bins.len()], hour.bins.as_ref());
        assert_eq!(hour.bins.last().map(|bin| bin.ceiling), Some(4.0));
    }
}
