use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};

pub use eternalist_apps::CabinetEntry as Named;

pub type Berth = eternalist_apps::CabinetBerth<EntryName>;
pub type ShelfBerth = eternalist_apps::CabinetShelfBerth;
pub type Library<T> = eternalist_apps::Cabinet<T>;

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct EntryName(String);

impl EntryName {
    pub fn forge(raw: &str) -> Option<Self> {
        let name = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        (!name.is_empty()).then_some(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for EntryName {
    fn default() -> Self {
        Self("default".to_owned())
    }
}

impl Display for EntryName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for EntryName {
    type Error = &'static str;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        Self::forge(&raw).ok_or("library entry name is empty")
    }
}

impl From<EntryName> for String {
    fn from(name: EntryName) -> Self {
        name.0
    }
}

impl eternalist_apps::CabinetKey for EntryName {
    fn forge(raw: &str) -> Option<Self> {
        Self::forge(raw)
    }

    fn as_str(&self) -> &str {
        self.as_str()
    }
}
