use crate::library::{Berth, EntryName, Library, Named, ShelfBerth};
use brass_poolrooms::{
    chrome::{self, MechanismSize, Monoglyph, Symbol},
    water::Surface,
};
use eternalist_apps::CabinetAction;

#[derive(Clone, Debug)]
pub enum Action<T> {
    New,
    BeginNameEdit,
    Rename,
    Load(T),
    Clone(EntryName),
    Delete(EntryName),
    RenameEntry { from: EntryName, to: EntryName },
    Moor { name: EntryName, berth: Berth },
    MoorShelf { shelf: usize, berth: ShelfBerth },
    NewShelf,
    ToggleShelf(usize),
    ScuttleShelf(usize),
    BeginShelfRename(usize),
    CommitShelfRename,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum NameEdit {
    #[default]
    Idle,
    Arming,
    Editing,
}

pub use eternalist_apps::CabinetShelfEdit as ShelfEdit;
pub type EntryEdit = eternalist_apps::CabinetEntryEdit<EntryName>;

pub fn active_card<T>(
    ui: &mut egui::Ui,
    water: &mut Surface,
    noun: &'static str,
    name_entry: &mut String,
    edit: &mut NameEdit,
    active: &EntryName,
) -> Vec<Action<T>> {
    let mut actions = Vec::new();
    let _title = ui.horizontal(|ui| {
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        let rename = Monoglyph::symbol(Symbol::Rename)
            .size(MechanismSize::Small)
            .show(ui)
            .on_hover_text("Rename");
        water.monoglyph(&rename);
        if rename.clicked() {
            actions.push(Action::BeginNameEdit);
        }
        if *edit == NameEdit::Idle {
            let _name = ui.label(chrome::title(active.to_string()));
        } else {
            let before = name_entry.clone();
            let entry = ui.add_sized(
                [ui.available_width(), 20.0],
                egui::TextEdit::singleline(name_entry).hint_text(format!("{noun} name")),
            );
            if let Some(wake) = chrome::text_wake(ui, &entry, &before, name_entry) {
                water.text(wake);
            }
            if *edit == NameEdit::Arming {
                entry.request_focus();
                *edit = NameEdit::Editing;
            }
            let enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
            if enter && (entry.has_focus() || entry.lost_focus()) {
                actions.push(Action::Rename);
            } else if entry.lost_focus() {
                *edit = NameEdit::Idle;
            }
        }
    });
    let _controls = ui.horizontal_wrapped(|ui| {
        let create = Monoglyph::symbol(Symbol::Add)
            .size(MechanismSize::Small)
            .show(ui)
            .on_hover_text(format!("New {noun}"));
        water.monoglyph(&create);
        if create.clicked() {
            actions.push(Action::New);
        }
    });
    actions
}

pub fn library<T: Named<Key = EntryName>>(
    ui: &mut egui::Ui,
    water: &mut Surface,
    noun: &'static str,
    active: &EntryName,
    bank: &Library<T>,
    shelf_edit: &mut Option<ShelfEdit>,
    entry_edit: &mut Option<EntryEdit>,
) -> Vec<Action<T>> {
    bank.show_renamable(
        ui,
        water,
        "views",
        noun,
        Some(active),
        shelf_edit,
        entry_edit,
    )
    .into_iter()
    .map(Action::from)
    .collect()
}

impl<T: Named<Key = EntryName>> From<CabinetAction<T>> for Action<T> {
    fn from(action: CabinetAction<T>) -> Self {
        match action {
            CabinetAction::Load(entry) => Self::Load(entry),
            CabinetAction::Clone(name) => Self::Clone(name),
            CabinetAction::Delete(name) => Self::Delete(name),
            CabinetAction::RenameEntry { from, to } => Self::RenameEntry { from, to },
            CabinetAction::Moor { key: name, berth } => Self::Moor { name, berth },
            CabinetAction::MoorShelf { shelf, berth } => Self::MoorShelf { shelf, berth },
            CabinetAction::NewShelf => Self::NewShelf,
            CabinetAction::ToggleShelf(shelf) => Self::ToggleShelf(shelf),
            CabinetAction::ScuttleShelf(shelf) => Self::ScuttleShelf(shelf),
            CabinetAction::BeginShelfRename(shelf) => Self::BeginShelfRename(shelf),
            CabinetAction::CommitShelfRename => Self::CommitShelfRename,
        }
    }
}
