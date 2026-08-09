//! Tester-independent vocabulary shared across HRRR's native UI boundary.

use std::{borrow::Cow, fmt};

pub const UI_FINGERPRINT: &str = "hrrr.ui/1";

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Target {
    Map,
    Pin(usize),
    TransientProbe,
}

impl Target {
    #[must_use]
    pub fn wire(self) -> Cow<'static, str> {
        match self {
            Self::Map => Cow::Borrowed("map.canvas"),
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

    #[test]
    fn indexed_map_objects_have_disjoint_wire_identity() {
        assert_ne!(Target::Pin(0).wire(), Target::Pin(1).wire());
        assert_ne!(Target::Pin(0).wire(), Target::TransientProbe.wire());
    }
}
