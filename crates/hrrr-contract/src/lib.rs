//! Tester-independent vocabulary shared across HRRR's native UI boundary.

use std::{borrow::Cow, fmt};

pub const UI_FINGERPRINT: &str = "hrrr.ui/2";

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Target {
    Field(&'static str),
    Map,
    Pin(usize),
    TransientProbe,
}

impl Target {
    #[must_use]
    pub fn wire(self) -> Cow<'static, str> {
        match self {
            Self::Field(name) => Cow::Owned(format!("field/{name}")),
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

    #[test]
    fn fields_have_stable_disjoint_wire_identity() {
        assert_eq!(Target::Field("cloud-cover").wire(), "field/cloud-cover");
        assert_ne!(
            Target::Field("cloud-cover").wire(),
            Target::Field("smoke").wire()
        );
    }
}
