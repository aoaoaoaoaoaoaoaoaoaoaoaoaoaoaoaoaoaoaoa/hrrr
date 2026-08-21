//! Tester-independent vocabulary shared across HRRR's native UI boundary.

use std::{borrow::Cow, fmt};

pub const UI_FINGERPRINT: &str = "hrrr.ui/6";

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum Target {
    BasemapInstall,
    BaseHour,
    CommandGuide,
    Field(&'static str),
    ForecastHour,
    Help,
    Map,
    Panel(&'static str),
    Pin(usize),
    TransientProbe,
}

impl Target {
    #[must_use]
    pub fn wire(self) -> Cow<'static, str> {
        match self {
            Self::BasemapInstall => Cow::Borrowed("basemap.install"),
            Self::BaseHour => Cow::Borrowed("forecast.base-hour"),
            Self::CommandGuide => Cow::Borrowed("application.command-guide"),
            Self::Field(name) => Cow::Owned(format!("field/{name}")),
            Self::ForecastHour => Cow::Borrowed("forecast.valid-hour"),
            Self::Help => Cow::Borrowed("application.help"),
            Self::Map => Cow::Borrowed("map.canvas"),
            Self::Panel(name) => Cow::Owned(format!("panel/{name}")),
            Self::Pin(slot) => Cow::Owned(format!("map.pin/{slot}")),
            Self::TransientProbe => Cow::Borrowed("map.probe/transient"),
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
            (Target::BasemapInstall, "basemap.install"),
            (Target::BaseHour, "forecast.base-hour"),
            (Target::CommandGuide, "application.command-guide"),
            (Target::Field("cloud-cover"), "field/cloud-cover"),
            (Target::Field("smoke"), "field/smoke"),
            (Target::ForecastHour, "forecast.valid-hour"),
            (Target::Help, "application.help"),
            (Target::Map, "map.canvas"),
            (Target::Panel("forecast"), "panel/forecast"),
            (Target::Pin(0), "map.pin/0"),
            (Target::Pin(1), "map.pin/1"),
            (Target::TransientProbe, "map.probe/transient"),
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
