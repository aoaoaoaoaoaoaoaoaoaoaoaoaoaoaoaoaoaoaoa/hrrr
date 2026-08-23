use crate::model::FieldGrid;
use anyhow::Result;

#[derive(Clone, Copy)]
struct Breakpoint {
    concentration: [f32; 2],
    index: [f32; 2],
}

impl Breakpoint {
    const fn new(concentration: [f32; 2], index: [f32; 2]) -> Self {
        Self {
            concentration,
            index,
        }
    }

    fn contains(self, concentration: f32) -> bool {
        (self.concentration[0]..=self.concentration[1]).contains(&concentration)
    }

    fn index(self, concentration: f32) -> f32 {
        let [lo, hi] = self.concentration;
        let [index_lo, index_hi] = self.index;
        ((index_hi - index_lo) / (hi - lo)).mul_add(concentration - lo, index_lo)
    }
}

const PM25: [Breakpoint; 6] = [
    Breakpoint::new([0.0, 9.0], [0.0, 50.0]),
    Breakpoint::new([9.1, 35.4], [51.0, 100.0]),
    Breakpoint::new([35.5, 55.4], [101.0, 150.0]),
    Breakpoint::new([55.5, 125.4], [151.0, 200.0]),
    Breakpoint::new([125.5, 225.4], [201.0, 300.0]),
    Breakpoint::new([225.5, 325.4], [301.0, 500.0]),
];

const OZONE_EIGHT_HOUR: [Breakpoint; 5] = [
    Breakpoint::new([0.0, 54.0], [0.0, 50.0]),
    Breakpoint::new([55.0, 70.0], [51.0, 100.0]),
    Breakpoint::new([71.0, 85.0], [101.0, 150.0]),
    Breakpoint::new([86.0, 105.0], [151.0, 200.0]),
    Breakpoint::new([106.0, 200.0], [201.0, 300.0]),
];

const OZONE_ONE_HOUR: [Breakpoint; 5] = [
    Breakpoint::new([125.0, 164.0], [101.0, 150.0]),
    Breakpoint::new([165.0, 204.0], [151.0, 200.0]),
    Breakpoint::new([205.0, 404.0], [201.0, 300.0]),
    Breakpoint::new([405.0, 504.0], [301.0, 400.0]),
    Breakpoint::new([505.0, 604.0], [401.0, 500.0]),
];

pub(crate) fn field(
    fine_particulate: &FieldGrid,
    ozone_eight_hour: &FieldGrid,
    ozone_one_hour: &FieldGrid,
) -> Result<FieldGrid> {
    FieldGrid::fuse3(
        fine_particulate,
        ozone_eight_hour,
        ozone_one_hour,
        |pm25, ozone8, ozone1| {
            [pm25_aqi(pm25), ozone_aqi(ozone8, ozone1)]
                .into_iter()
                .flatten()
                .reduce(f32::max)
                .unwrap_or(f32::NAN)
        },
    )
}

fn pm25_aqi(concentration: f32) -> Option<f32> {
    let concentration = truncate(concentration, 10.0)?;
    interpolate(concentration, &PM25).map(f32::round)
}

fn ozone_aqi(eight_hour_ppb: f32, one_hour_ppb: f32) -> Option<f32> {
    let eight_hour = truncate(eight_hour_ppb, 1.0)
        .filter(|value| *value <= 200.0)
        .and_then(|value| interpolate(value, &OZONE_EIGHT_HOUR));
    let one_hour = truncate(one_hour_ppb, 1.0)
        .filter(|value| *value >= 125.0)
        .and_then(|value| interpolate(value, &OZONE_ONE_HOUR));
    [eight_hour, one_hour]
        .into_iter()
        .flatten()
        .reduce(f32::max)
        .map(f32::round)
}

fn truncate(value: f32, precision: f32) -> Option<f32> {
    value
        .is_finite()
        .then(|| (value.max(0.0) * precision).floor() / precision)
}

fn interpolate(concentration: f32, breakpoints: &[Breakpoint]) -> Option<f32> {
    breakpoints
        .iter()
        .copied()
        .find(|breakpoint| breakpoint.contains(concentration))
        .or_else(|| {
            breakpoints
                .last()
                .copied()
                .filter(|_| concentration.is_finite())
        })
        .map(|breakpoint| breakpoint.index(concentration))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epa_breakpoints_preserve_truncation_and_pollutant_dominance() {
        assert_eq!(pm25_aqi(9.09), Some(50.0));
        assert_eq!(pm25_aqi(9.10), Some(51.0));
        assert_eq!(pm25_aqi(325.4), Some(500.0));
        assert_eq!(pm25_aqi(326.0), Some(501.0));
        assert_eq!(ozone_aqi(70.9, 124.9), Some(100.0));
        assert_eq!(ozone_aqi(71.0, 124.9), Some(101.0));
        assert_eq!(ozone_aqi(70.0, 205.0), Some(201.0));
        assert_eq!(pm25_aqi(f32::NAN), None);
    }
}
