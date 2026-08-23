//! Tester-independent vocabulary shared across HRRR's native UI boundary.

use std::{borrow::Cow, fmt};

pub const UI_FINGERPRINT: &str = "hrrr.ui/7";

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum Target {
    BasemapInstall,
    BaseHour,
    Field(&'static str),
    ForecastHour,
    Map,
    Panel(&'static str),
    Pin(usize),
    TransientProbe,
}

impl Target {
    #[must_use]
    pub fn wire(self) -> Cow<'static, str> {
        match self {
            Self::BasemapInstall => Cow::Borrowed("hrrr.basemap.install"),
            Self::BaseHour => Cow::Borrowed("hrrr.forecast.base-hour"),
            Self::Field(name) => Cow::Owned(format!("hrrr.field/{name}")),
            Self::ForecastHour => Cow::Borrowed("hrrr.forecast.valid-hour"),
            Self::Map => Cow::Borrowed("hrrr.map.canvas"),
            Self::Panel(name) => Cow::Owned(format!("hrrr.inspector.panel/{name}")),
            Self::Pin(slot) => Cow::Owned(format!("hrrr.map.pin/{slot}")),
            Self::TransientProbe => Cow::Borrowed("hrrr.map.probe.transient"),
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.wire())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn wire_vocabulary_is_stable_and_disjoint() {
        let vocabulary = [
            (Target::BasemapInstall, "hrrr.basemap.install"),
            (Target::BaseHour, "hrrr.forecast.base-hour"),
            (Target::Field("cloud-cover"), "hrrr.field/cloud-cover"),
            (Target::Field("smoke"), "hrrr.field/smoke"),
            (Target::ForecastHour, "hrrr.forecast.valid-hour"),
            (Target::Map, "hrrr.map.canvas"),
            (Target::Panel("forecast"), "hrrr.inspector.panel/forecast"),
            (Target::Pin(0), "hrrr.map.pin/0"),
            (Target::Pin(1), "hrrr.map.pin/1"),
            (Target::TransientProbe, "hrrr.map.probe.transient"),
        ];
        let wires = vocabulary
            .iter()
            .map(|(target, expected)| {
                let wire = target.wire();
                assert_eq!(wire, *expected);
                wire.into_owned()
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(wires.len(), vocabulary.len());
    }
}
