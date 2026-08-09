use crate::{
    library::{EntryName, Library, Named},
    model::{MercatorPoint, Viewport},
    persist::{load_toml, save_toml},
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fmt::{Display, Formatter},
    ops::{Deref, DerefMut},
    path::Path,
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct ViewSlot(u8);

impl ViewSlot {
    pub const fn forge(digit: u8) -> Option<Self> {
        if digit <= 9 { Some(Self(digit)) } else { None }
    }

    pub const fn digit(self) -> u8 {
        self.0
    }

    pub const fn sigil(self) -> char {
        (b'0' + self.0) as char
    }
}

impl Display for ViewSlot {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl TryFrom<u8> for ViewSlot {
    type Error = &'static str;

    fn try_from(digit: u8) -> Result<Self, Self::Error> {
        Self::forge(digit).ok_or("view slot must be a decimal digit")
    }
}

impl From<ViewSlot> for u8 {
    fn from(slot: ViewSlot) -> Self {
        slot.0
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SavedView {
    pub name: EntryName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<ViewSlot>,
    pub viewport: Viewport,
    #[serde(alias = "probes")]
    pub pins: Vec<MercatorPoint>,
}

impl SavedView {
    pub fn forge(name: EntryName, mut viewport: Viewport, pins: Vec<MercatorPoint>) -> Self {
        viewport.normalize();
        Self {
            name,
            slot: None,
            viewport,
            pins,
        }
    }

    pub fn default_view() -> Self {
        Self::forge(EntryName::default(), Viewport::default(), Vec::new())
    }

    pub fn normalize(&mut self) {
        self.viewport.normalize();
        self.pins = self
            .pins
            .drain(..)
            .filter_map(MercatorPoint::normalize)
            .collect();
    }

    pub fn reframe(&mut self, viewport: Viewport, pins: Vec<MercatorPoint>) {
        self.viewport = viewport;
        self.pins = pins;
        self.normalize();
    }
}

impl Named for SavedView {
    type Key = EntryName;

    fn key(&self) -> &EntryName {
        &self.name
    }

    fn rename(&mut self, name: EntryName) {
        self.name = name;
    }

    fn sigil(&self) -> Option<char> {
        self.slot.map(ViewSlot::sigil)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ViewLibrary(Library<SavedView>);

impl Default for ViewLibrary {
    fn default() -> Self {
        Self(Library {
            saved: vec![SavedView::default_view()],
            shelves: Vec::new(),
        })
    }
}

impl Deref for ViewLibrary {
    type Target = Library<SavedView>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for ViewLibrary {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl ViewLibrary {
    pub fn load(path: &Path, legacy: Option<Self>) -> Result<(Self, bool)> {
        let (mut views, migrated) = match load_toml(path, "view library")? {
            Some(views) => (views, false),
            None => (legacy.unwrap_or_default(), true),
        };
        views.rectify();
        Ok((views, migrated))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        save_toml(self, path, "serialize view library")
    }

    fn rectify(&mut self) {
        self.0.rectify();
        let mut claimed = BTreeSet::new();
        for view in self.all_mut() {
            view.normalize();
            if let Some(slot) = view.slot
                && !claimed.insert(slot)
            {
                view.slot = None;
            }
        }
        if self.all().next().is_none() {
            self.saved.push(SavedView::default_view());
        }
    }
}
