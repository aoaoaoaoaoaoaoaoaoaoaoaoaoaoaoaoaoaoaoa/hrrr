//! Tester-independent vocabulary shared across HRRR's native UI boundary.

use std::{borrow::Cow, fmt};

/// Reverse-DNS product identifier; the sole source of HRRR's platform and crash identity.
pub const PRODUCT_IDENTIFIER: &str = "moe.swarm.hrrr";
/// Product display name.
pub const PRODUCT_NAME: &str = "HRRR";
pub const UI_FINGERPRINT: &str = "hrrr.ui/8";

/// One Inspector Panel, in carousel order.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(rename_all = "snake_case")
)]
pub enum Panel {
    Application,
    Field,
    Forecast,
    ActiveView,
    Views,
    Status,
}

impl Panel {
    pub const ALL: [Self; 6] = [
        Self::Application,
        Self::Field,
        Self::Forecast,
        Self::ActiveView,
        Self::Views,
        Self::Status,
    ];

    /// Kebab-case identity on the witness wire.
    #[must_use]
    pub const fn wire(self) -> &'static str {
        match self {
            Self::Application => "application",
            Self::Field => "field",
            Self::Forecast => "forecast",
            Self::ActiveView => "active-view",
            Self::Views => "views",
            Self::Status => "status",
        }
    }

    /// Position in carousel order.
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }
}

/// One forecast field a person may select.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Field {
    QpfTotal,
    QpfHourly,
    SurfaceSmoke,
    Temperature,
    DewPoint,
    CloudCover,
    SeaLevelPressure,
    Wind,
    AirQuality,
}

impl Field {
    pub const ALL: [Self; 9] = [
        Self::QpfTotal,
        Self::QpfHourly,
        Self::SurfaceSmoke,
        Self::Temperature,
        Self::DewPoint,
        Self::CloudCover,
        Self::SeaLevelPressure,
        Self::Wind,
        Self::AirQuality,
    ];

    /// Kebab-case identity on the witness wire, also HRRR's cache name.
    #[must_use]
    pub const fn wire(self) -> &'static str {
        match self {
            Self::QpfTotal => "qpf",
            Self::QpfHourly => "qpf-hour",
            Self::SurfaceSmoke => "smoke",
            Self::Temperature => "temperature",
            Self::DewPoint => "dew-point",
            Self::CloudCover => "cloud-cover",
            Self::SeaLevelPressure => "pressure",
            Self::Wind => "wind",
            Self::AirQuality => "air-quality",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum Target {
    BasemapInstall,
    BaseHour,
    Field(Field),
    ForecastHour,
    Map,
    Panel(Panel),
    Pin(usize),
    PinRemove(usize),
    RunNewer,
    RunOlder,
    TransientProbe,
    ValidHourEarlier,
    ValidHourLater,
    ViewCreate,
    ViewName,
    ViewRename,
}

impl Target {
    #[must_use]
    pub fn wire(self) -> Cow<'static, str> {
        match self {
            Self::BasemapInstall => Cow::Borrowed("hrrr.basemap.install"),
            Self::BaseHour => Cow::Borrowed("hrrr.forecast.base-hour"),
            Self::Field(field) => Cow::Owned(format!("hrrr.field/{}", field.wire())),
            Self::ForecastHour => Cow::Borrowed("hrrr.forecast.valid-hour"),
            Self::Map => Cow::Borrowed("hrrr.map.canvas"),
            Self::Panel(panel) => Cow::Owned(format!("hrrr.inspector.panel/{}", panel.wire())),
            Self::Pin(slot) => Cow::Owned(format!("hrrr.map.pin/{slot}")),
            Self::PinRemove(slot) => Cow::Owned(format!("hrrr.map.pin.remove/{slot}")),
            Self::RunNewer => Cow::Borrowed("hrrr.forecast.run.newer"),
            Self::RunOlder => Cow::Borrowed("hrrr.forecast.run.older"),
            Self::TransientProbe => Cow::Borrowed("hrrr.map.probe.transient"),
            Self::ValidHourEarlier => Cow::Borrowed("hrrr.forecast.valid-hour.earlier"),
            Self::ValidHourLater => Cow::Borrowed("hrrr.forecast.valid-hour.later"),
            Self::ViewCreate => Cow::Borrowed("hrrr.view.create"),
            Self::ViewName => Cow::Borrowed("hrrr.view.name"),
            Self::ViewRename => Cow::Borrowed("hrrr.view.rename"),
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
    fn wire_vocabulary_is_disjoint() {
        let targets = [
            Target::BasemapInstall,
            Target::BaseHour,
            Target::ForecastHour,
            Target::Map,
            Target::Pin(0),
            Target::Pin(1),
            Target::PinRemove(0),
            Target::PinRemove(1),
            Target::RunNewer,
            Target::RunOlder,
            Target::TransientProbe,
            Target::ValidHourEarlier,
            Target::ValidHourLater,
            Target::ViewCreate,
            Target::ViewName,
            Target::ViewRename,
        ]
        .into_iter()
        .chain(Field::ALL.map(Target::Field))
        .chain(Panel::ALL.map(Target::Panel))
        .collect::<Vec<_>>();
        let wires = targets
            .iter()
            .map(|target| target.wire().into_owned())
            .collect::<BTreeSet<_>>();
        assert_eq!(wires.len(), targets.len());
    }
}
