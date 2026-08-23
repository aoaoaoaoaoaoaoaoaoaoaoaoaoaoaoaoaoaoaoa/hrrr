use crate::model::{FrameKey, Product};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Unit {
    Inch,
    MicrogramPerCubicMetre,
    Fahrenheit,
    Percent,
    MilesPerHour,
}

impl Unit {
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::Inch => "in",
            Self::MicrogramPerCubicMetre => "µg/m³",
            Self::Fahrenheit => "°F",
            Self::Percent => "%",
            Self::MilesPerHour => "mph",
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
        }
    }

    pub fn format(self, value: f32) -> String {
        match self {
            Self::Inch => format!("{value:.2} {}", self.symbol()),
            Self::MicrogramPerCubicMetre => format!("{value:.1} {}", self.symbol()),
            Self::Fahrenheit => format!("{value:.1}{}", self.symbol()),
            Self::Percent => format!("{value:.0}{}", self.symbol()),
            Self::MilesPerHour => format!("{value:.1} {}", self.symbol()),
        }
    }

    pub fn format_ceiling(self, value: f32) -> String {
        match self {
            Self::Percent | Self::Fahrenheit => format!("{value:.0}{}", self.symbol()),
            Self::Inch | Self::MicrogramPerCubicMetre | Self::MilesPerHour => {
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
pub enum SmokeRegime {
    #[default]
    Light,
    Heavy,
}

impl SmokeRegime {
    const HEAVY_ONSET: f32 = 40.0;
    const LIGHT_RETURN: f32 = 20.0;

    pub fn reckon(self, visible_peak: Option<f32>) -> Self {
        let Some(visible_peak) = visible_peak else {
            return self;
        };
        match self {
            Self::Light if visible_peak > Self::HEAVY_ONSET => Self::Heavy,
            Self::Heavy if visible_peak < Self::LIGHT_RETURN => Self::Light,
            _ => self,
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
    Smoke { light: Scale, heavy: Scale },
    Temperature { summer: Scale, winter: Scale },
}

impl ScaleFamily {
    fn resolve(&self, smoke: SmokeRegime, temperature: TemperatureSeason) -> &Scale {
        match (self, smoke, temperature) {
            (Self::Static(scale), _, _) => scale,
            (Self::Smoke { light, .. }, SmokeRegime::Light, _) => light,
            (Self::Smoke { heavy, .. }, SmokeRegime::Heavy, _) => heavy,
            (Self::Temperature { summer, .. }, _, TemperatureSeason::Summer) => summer,
            (Self::Temperature { winter, .. }, _, TemperatureSeason::Winter) => winter,
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
            pub fn get(
                &self,
                product: Product,
                smoke: SmokeRegime,
                temperature: TemperatureSeason,
            ) -> &Scale {
                match product {
                    $(Product::$product => &self.$field),+
                }
                .resolve(smoke, temperature)
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
    Smoke => smoke = ScaleFamily::Smoke {
        light: Scale::forge(Unit::MicrogramPerCubicMetre, LIGHT_SMOKE, SMOKE_CONTOUR),
        heavy: Scale::forge(Unit::MicrogramPerCubicMetre, HEAVY_SMOKE, SMOKE_CONTOUR),
    },
    Temperature => temperature = ScaleFamily::Temperature {
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
    Wind => wind = ScaleFamily::Static(Scale::forge(
        Unit::MilesPerHour,
        WIND,
        WIND_CONTOUR,
    )),
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
const WIND_CONTOUR: Contour = Contour {
    width_points: 0.22,
    srgb: [35, 31, 26, 46],
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
const SMOKE_0: Bin = Bin::new(0.0, VOID);
const SMOKE_0_1: Bin = Bin::new(0.1, [132, 161, 179, 58]);
const SMOKE_0_25: Bin = Bin::new(0.25, [112, 151, 176, 69]);
const SMOKE_0_5: Bin = Bin::new(0.5, [92, 140, 169, 81]);
const SMOKE_1: Bin = Bin::new(1.0, [73, 127, 158, 95]);
const SMOKE_2: Bin = Bin::new(2.0, [76, 132, 144, 109]);
const SMOKE_5: Bin = Bin::new(5.0, [142, 145, 102, 126]);
const SMOKE_10: Bin = Bin::new(10.0, [195, 155, 78, 148]);
const SMOKE_20: Bin = Bin::new(20.0, [219, 112, 58, 176]);
const SMOKE_40: Bin = Bin::new(40.0, [194, 67, 56, 201]);
const SMOKE_80: Bin = Bin::new(80.0, [147, 52, 85, 221]);
const SMOKE_160: Bin = Bin::new(160.0, [91, 48, 103, 236]);
const SMOKE_320: Bin = Bin::new(320.0, [39, 31, 42, 246]);
const LIGHT_SMOKE: [Bin; 10] = [
    SMOKE_0, SMOKE_0_1, SMOKE_0_25, SMOKE_0_5, SMOKE_1, SMOKE_2, SMOKE_5, SMOKE_10, SMOKE_20,
    SMOKE_40,
];
const HEAVY_SMOKE: [Bin; 9] = [
    SMOKE_0, SMOKE_1, SMOKE_5, SMOKE_10, SMOKE_20, SMOKE_40, SMOKE_80, SMOKE_160, SMOKE_320,
];
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
    }

    #[test]
    fn defaults_are_strictly_ordered() {
        let atlas = ScaleAtlas::default();
        for product in Product::ALL {
            for regime in [SmokeRegime::Light, SmokeRegime::Heavy] {
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
    fn smoke_regimes_are_fixed_consistent_and_hysteretic() {
        let atlas = ScaleAtlas::default();
        let light = atlas.get(
            Product::Smoke,
            SmokeRegime::Light,
            TemperatureSeason::Summer,
        );
        let heavy = atlas.get(
            Product::Smoke,
            SmokeRegime::Heavy,
            TemperatureSeason::Summer,
        );
        for shared in [0.0, 1.0, 5.0, 10.0, 20.0, 40.0] {
            let light_color = light
                .bins
                .iter()
                .find(|bin| bin.ceiling == shared)
                .map(|bin| bin.srgb);
            let heavy_color = heavy
                .bins
                .iter()
                .find(|bin| bin.ceiling == shared)
                .map(|bin| bin.srgb);
            assert_eq!(light_color, heavy_color);
        }
        assert_eq!(SmokeRegime::Light.reckon(Some(40.0)), SmokeRegime::Light);
        assert_eq!(SmokeRegime::Light.reckon(Some(40.1)), SmokeRegime::Heavy);
        assert_eq!(SmokeRegime::Heavy.reckon(Some(20.0)), SmokeRegime::Heavy);
        assert_eq!(SmokeRegime::Heavy.reckon(Some(19.9)), SmokeRegime::Light);
    }

    #[test]
    fn qpf_scales_share_one_value_color_language() {
        let atlas = ScaleAtlas::default();
        let run = atlas.get(
            Product::QpfRun,
            SmokeRegime::Light,
            TemperatureSeason::Summer,
        );
        let hour = atlas.get(
            Product::QpfHour,
            SmokeRegime::Light,
            TemperatureSeason::Summer,
        );
        assert_eq!(run.bins.len(), 18);
        assert_eq!(hour.bins.len(), QPF_HOURLY_BINS);
        assert_eq!(&run.bins[..hour.bins.len()], hour.bins.as_ref());
        assert_eq!(hour.bins.last().map(|bin| bin.ceiling), Some(4.0));
    }
}
