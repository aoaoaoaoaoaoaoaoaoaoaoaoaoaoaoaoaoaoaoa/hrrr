use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashSet},
    fmt::{Display, Formatter},
};

#[derive(Clone, Debug)]
pub enum Berth {
    Beside { anchor: EntryName, after: bool },
    Shelf(usize),
    Root,
}

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

pub trait Named {
    fn name(&self) -> &EntryName;
    fn rename(&mut self, name: EntryName);

    fn sigil(&self) -> Option<char> {
        None
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Shelf<T> {
    pub name: String,
    #[serde(skip, default = "shelf_open")]
    pub open: bool,
    pub entries: Vec<T>,
}

impl<T> Default for Shelf<T> {
    fn default() -> Self {
        Self {
            name: String::new(),
            open: true,
            entries: Vec::new(),
        }
    }
}

const fn shelf_open() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Library<T> {
    pub saved: Vec<T>,
    pub shelves: Vec<Shelf<T>>,
}

impl<T> Default for Library<T> {
    fn default() -> Self {
        Self {
            saved: Vec::new(),
            shelves: Vec::new(),
        }
    }
}

impl<T: Named> Library<T> {
    pub fn rectify(&mut self) {
        let mut seen = HashSet::new();
        for entry in &mut self.saved {
            rectify_entry_name(entry, &mut seen);
        }
        let mut shelf_names = HashSet::new();
        for shelf in &mut self.shelves {
            let base = normalized_shelf_name(&shelf.name);
            let mut name = base.clone();
            let mut suffix = 2_u64;
            while !shelf_names.insert(name.clone()) {
                name = format!("{base} {suffix}");
                suffix = suffix.saturating_add(1);
            }
            shelf.name = name;
            for entry in &mut shelf.entries {
                rectify_entry_name(entry, &mut seen);
            }
        }
    }

    pub fn all(&self) -> impl Iterator<Item = &T> {
        self.saved
            .iter()
            .chain(self.shelves.iter().flat_map(|shelf| shelf.entries.iter()))
    }

    pub fn all_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.saved.iter_mut().chain(
            self.shelves
                .iter_mut()
                .flat_map(|shelf| shelf.entries.iter_mut()),
        )
    }

    pub fn get(&self, name: &EntryName) -> Option<&T> {
        self.all().find(|entry| entry.name() == name)
    }

    pub fn get_mut(&mut self, name: &EntryName) -> Option<&mut T> {
        self.find_mut(name)
    }

    pub fn taken(&self, name: &EntryName) -> bool {
        self.get(name).is_some()
    }

    pub fn active(&self, active: Option<EntryName>) -> Option<EntryName> {
        active.filter(|name| self.taken(name))
    }

    pub fn upsert(&mut self, entry: T) {
        match self.find_mut(entry.name()) {
            Some(slot) => *slot = entry,
            None => self.saved.push(entry),
        }
    }

    pub fn remove(&mut self, name: &EntryName) -> Option<T> {
        if let Some(slot) = self.saved.iter().position(|entry| entry.name() == name) {
            return Some(self.saved.remove(slot));
        }
        self.shelves.iter_mut().find_map(|shelf| {
            shelf
                .entries
                .iter()
                .position(|entry| entry.name() == name)
                .map(|slot| shelf.entries.remove(slot))
        })
    }

    pub fn rename(&mut self, old: &EntryName, new: EntryName) {
        if let Some(entry) = self.find_mut(old) {
            entry.rename(new);
        }
    }

    pub fn moor(&mut self, name: &EntryName, berth: &Berth) {
        if let Berth::Beside { anchor, .. } = berth
            && anchor == name
        {
            return;
        }
        let Some(entry) = self.remove(name) else {
            return;
        };
        match berth {
            Berth::Beside { anchor, after } => {
                let slip = usize::from(*after);
                match self.berth_of(anchor) {
                    Some((None, slot)) => self.saved.insert(slot + slip, entry),
                    Some((Some(shelf), slot)) => {
                        self.shelves[shelf].entries.insert(slot + slip, entry);
                    }
                    None => self.saved.push(entry),
                }
            }
            Berth::Shelf(shelf) => match self.shelves.get_mut(*shelf) {
                Some(shelf) => shelf.entries.push(entry),
                None => self.saved.push(entry),
            },
            Berth::Root => self.saved.push(entry),
        }
    }

    pub fn add_shelf(&mut self) {
        let mut name = "folder".to_owned();
        let mut suffix = 2_u64;
        while self.shelves.iter().any(|shelf| shelf.name == name) {
            name = format!("folder {suffix}");
            suffix = suffix.saturating_add(1);
        }
        self.shelves.push(Shelf {
            name,
            open: true,
            entries: Vec::new(),
        });
    }

    pub fn toggle_shelf(&mut self, shelf: usize) {
        if let Some(shelf) = self.shelves.get_mut(shelf) {
            shelf.open = !shelf.open;
        }
    }

    pub fn restore_folds(&mut self, closed: &BTreeSet<String>) {
        for shelf in &mut self.shelves {
            shelf.open = !closed.contains(&shelf.name);
        }
    }

    pub fn closed_folders(&self) -> BTreeSet<String> {
        self.shelves
            .iter()
            .filter(|shelf| !shelf.open)
            .map(|shelf| shelf.name.clone())
            .collect()
    }

    pub fn adopt_beside(&mut self, anchor: &EntryName, entry: T) {
        match self.berth_of(anchor) {
            Some((None, slot)) => self.saved.insert(slot + 1, entry),
            Some((Some(shelf), slot)) => self.shelves[shelf].entries.insert(slot + 1, entry),
            None => self.saved.push(entry),
        }
    }

    pub fn scuttle_shelf(&mut self, shelf: usize) {
        if shelf < self.shelves.len() {
            let shelf = self.shelves.remove(shelf);
            self.saved.extend(shelf.entries);
        }
    }

    pub fn rename_shelf(&mut self, shelf: usize, name: &str) -> bool {
        let name = normalized_shelf_name(name);
        if self
            .shelves
            .iter()
            .enumerate()
            .any(|(slot, candidate)| slot != shelf && candidate.name == name)
        {
            return false;
        }
        let Some(shelf) = self.shelves.get_mut(shelf) else {
            return false;
        };
        shelf.name = name;
        true
    }

    pub fn spare_named(&self, base: &EntryName) -> EntryName {
        if !self.taken(base) {
            return base.clone();
        }
        let mut suffix = 2_u64;
        loop {
            let raw = format!("{} {suffix}", base.as_str());
            if let Some(candidate) = EntryName::forge(&raw)
                && !self.taken(&candidate)
            {
                return candidate;
            }
            suffix = suffix.saturating_add(1);
        }
    }

    fn find_mut(&mut self, name: &EntryName) -> Option<&mut T> {
        self.saved
            .iter_mut()
            .chain(
                self.shelves
                    .iter_mut()
                    .flat_map(|shelf| shelf.entries.iter_mut()),
            )
            .find(|entry| entry.name() == name)
    }

    fn berth_of(&self, name: &EntryName) -> Option<(Option<usize>, usize)> {
        if let Some(slot) = self.saved.iter().position(|entry| entry.name() == name) {
            return Some((None, slot));
        }
        self.shelves.iter().enumerate().find_map(|(shelf, rack)| {
            rack.entries
                .iter()
                .position(|entry| entry.name() == name)
                .map(|slot| (Some(shelf), slot))
        })
    }
}

fn rectify_entry_name(entry: &mut impl Named, seen: &mut HashSet<EntryName>) {
    let base = entry.name().clone();
    if seen.insert(base.clone()) {
        return;
    }
    let mut suffix = 2_u64;
    loop {
        let candidate = EntryName(format!("{} {suffix}", base.as_str()));
        if seen.insert(candidate.clone()) {
            entry.rename(candidate);
            return;
        }
        suffix = suffix.saturating_add(1);
    }
}

fn normalized_shelf_name(raw: &str) -> String {
    let name = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        "folder".to_owned()
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{Context as _, Result};

    #[derive(Clone, Debug, PartialEq)]
    struct Mark(EntryName);

    impl Named for Mark {
        fn name(&self) -> &EntryName {
            &self.0
        }

        fn rename(&mut self, name: EntryName) {
            self.0 = name;
        }
    }

    #[test]
    fn rectification_and_active_selection_obey_global_names() -> Result<()> {
        let duplicate = mark("alpha")?;
        let mut library = Library {
            saved: vec![duplicate.clone(), mark("beta")?],
            shelves: vec![Shelf {
                name: "rack".to_owned(),
                open: true,
                entries: vec![duplicate],
            }],
        };
        library.rectify();
        assert_eq!(library.all().count(), 3);
        assert!(library.taken(&EntryName::forge("alpha 2").context("alpha 2")?));
        assert_eq!(
            library
                .active(EntryName::forge("alpha"))
                .as_ref()
                .map(EntryName::as_str),
            Some("alpha")
        );
        assert!(library.active(EntryName::forge("lost")).is_none());
        Ok(())
    }

    #[test]
    fn clone_names_take_the_first_free_suffix() -> Result<()> {
        let library = Library {
            saved: vec![mark("view")?, mark("view 2")?],
            shelves: Vec::new(),
        };
        let base = EntryName::forge("view").context("view name")?;
        assert_eq!(library.spare_named(&base).as_str(), "view 3");
        Ok(())
    }

    #[test]
    fn mooring_preserves_order_across_folders() -> Result<()> {
        let mut library = Library {
            saved: vec![mark("a")?, mark("b")?, mark("c")?],
            shelves: Vec::new(),
        };
        library.add_shelf();
        let c = EntryName::forge("c").context("c")?;
        library.moor(
            &c,
            &Berth::Beside {
                anchor: EntryName::forge("a").context("a")?,
                after: false,
            },
        );
        assert_eq!(names(&library.saved), ["c", "a", "b"]);
        library.moor(&c, &Berth::Shelf(0));
        assert_eq!(names(&library.saved), ["a", "b"]);
        assert_eq!(names(&library.shelves[0].entries), ["c"]);
        library.adopt_beside(&c, mark("c 2")?);
        library.scuttle_shelf(0);
        assert_eq!(names(&library.saved), ["a", "b", "c", "c 2"]);
        Ok(())
    }

    #[test]
    fn folder_folds_roundtrip_as_state() {
        let mut library: Library<Mark> = Library {
            saved: Vec::new(),
            shelves: vec![Shelf {
                name: "storms".to_owned(),
                open: false,
                entries: Vec::new(),
            }],
        };
        let closed = library.closed_folders();
        library.shelves[0].open = true;
        library.restore_folds(&closed);
        assert!(!library.shelves[0].open);
    }

    #[test]
    fn folders_are_normalized_and_globally_unique() {
        let mut library: Library<Mark> = Library {
            saved: Vec::new(),
            shelves: vec![
                Shelf {
                    name: "  storms  ".to_owned(),
                    ..Shelf::default()
                },
                Shelf {
                    name: "storms".to_owned(),
                    ..Shelf::default()
                },
            ],
        };
        library.rectify();
        assert_eq!(library.shelves[0].name, "storms");
        assert_eq!(library.shelves[1].name, "storms 2");
        assert!(!library.rename_shelf(1, " storms "));
        assert_eq!(library.shelves[1].name, "storms 2");
    }

    fn mark(name: &str) -> Result<Mark> {
        Ok(Mark(EntryName::forge(name).context("entry name")?))
    }

    fn names(entries: &[Mark]) -> Vec<&str> {
        entries.iter().map(|entry| entry.name().as_str()).collect()
    }
}
