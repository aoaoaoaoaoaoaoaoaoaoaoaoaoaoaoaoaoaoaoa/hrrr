use crate::library::{Berth, EntryName, Library, Named, Shelf};
use dwemer_poolrooms::chrome;

#[derive(Clone, Debug)]
pub enum Action<T> {
    New,
    BeginNameEdit,
    Rename,
    Load(T),
    Clone(EntryName),
    Delete(EntryName),
    Moor { name: EntryName, berth: Berth },
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

#[derive(Clone, Debug)]
pub struct ShelfEdit {
    pub shelf: usize,
    pub name: String,
    pub focus: bool,
}

pub fn active_card<T>(
    ui: &mut egui::Ui,
    noun: &'static str,
    name_entry: &mut String,
    edit: &mut NameEdit,
    active: &EntryName,
) -> Vec<Action<T>> {
    let mut actions = Vec::new();
    let _title = ui.horizontal(|ui| {
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        if chrome::icon(ui, "✎")
            .on_hover_text("rename in place")
            .clicked()
        {
            actions.push(Action::BeginNameEdit);
        }
        if *edit == NameEdit::Idle {
            let _name = ui.label(chrome::title(active.to_string()));
        } else {
            let entry = ui.add_sized(
                [ui.available_width(), 20.0],
                egui::TextEdit::singleline(name_entry).hint_text(format!("{noun} name")),
            );
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
        if chrome::icon(ui, "✚")
            .on_hover_text(format!("new {noun} from current"))
            .clicked()
        {
            actions.push(Action::New);
        }
    });
    actions
}

pub fn library<T: Clone + Named + 'static>(
    ui: &mut egui::Ui,
    noun: &'static str,
    active: &EntryName,
    bank: &Library<T>,
    shelf_edit: &mut Option<ShelfEdit>,
) -> Vec<Action<T>> {
    let mut actions = Vec::new();
    for entry in &bank.saved {
        entry_row(ui, noun, active, entry, &mut actions);
    }
    for (slot, shelf) in bank.shelves.iter().enumerate() {
        shelf_rows(ui, noun, slot, shelf, active, shelf_edit, &mut actions);
    }
    let _controls = ui.horizontal_wrapped(|ui| {
        if chrome::icon(ui, "⊞").on_hover_text("new folder").clicked() {
            actions.push(Action::NewShelf);
        }
    });
    root_basin(ui, &mut actions);
    actions
}

fn entry_row<T: Clone + Named + 'static>(
    ui: &mut egui::Ui,
    noun: &'static str,
    active: &EntryName,
    entry: &T,
    actions: &mut Vec<Action<T>>,
) {
    let row = ui.horizontal(|ui| {
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        let name = entry.name();
        let drag_id = egui::Id::new(("library-drag", name.as_str()));
        let _handle = ui.dnd_drag_source(drag_id, name.clone(), |ui| {
            let _grip = ui
                .label(
                    egui::RichText::new("⠿")
                        .size(17.0)
                        .color(chrome::EDGE_STRONG),
                )
                .on_hover_text("drag to rearrange");
        });
        if chrome::icon(ui, "×")
            .on_hover_text(format!("delete {noun}"))
            .clicked()
        {
            actions.push(Action::Delete(name.clone()));
        }
        if chrome::icon(ui, "⧉")
            .on_hover_text(format!("clone {noun}"))
            .clicked()
        {
            actions.push(Action::Clone(name.clone()));
        }
        let selected = active == name;
        let sigil = entry.sigil().map(|sigil| format!("[{sigil}] "));
        let label = format!(
            "{}{}{}",
            if selected { "● " } else { "" },
            sigil.as_deref().unwrap_or_default(),
            name
        );
        let font = egui::TextStyle::Button.resolve(ui.style());
        let natural = ui
            .painter()
            .layout_no_wrap(label.clone(), font, egui::Color32::PLACEHOLDER)
            .size()
            .x;
        let truncated = natural > ui.available_width();
        let response = ui.selectable_label(selected, label);
        let response = if truncated {
            response.on_hover_text(name.as_str())
        } else {
            response
        };
        if response.clicked() {
            actions.push(Action::Load(entry.clone()));
        }
    });
    let rect = row.response.rect;
    let after = ui
        .ctx()
        .pointer_latest_pos()
        .is_some_and(|position| position.y > rect.center().y);
    if let Some(payload) = row.response.dnd_hover_payload::<EntryName>()
        && *payload != *entry.name()
    {
        let y = if after { rect.bottom() } else { rect.top() };
        let _line = ui
            .painter()
            .hline(rect.x_range(), y, egui::Stroke::new(1.0_f32, chrome::HOT));
    }
    if let Some(payload) = row.response.dnd_release_payload::<EntryName>()
        && *payload != *entry.name()
    {
        actions.push(Action::Moor {
            name: (*payload).clone(),
            berth: Berth::Beside {
                anchor: entry.name().clone(),
                after,
            },
        });
    }
}

fn shelf_rows<T: Clone + Named + 'static>(
    ui: &mut egui::Ui,
    noun: &'static str,
    slot: usize,
    shelf: &Shelf<T>,
    active: &EntryName,
    shelf_edit: &mut Option<ShelfEdit>,
    actions: &mut Vec<Action<T>>,
) {
    let id = ui.make_persistent_id(("library-shelf", slot));
    let header = ui.horizontal(|ui| {
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        let toggle = chrome::glyph(ui, if shelf.open { "▾" } else { "▸" }, false);
        if toggle.clicked() {
            actions.push(Action::ToggleShelf(slot));
        }
        if chrome::icon(ui, "✎")
            .on_hover_text("rename folder")
            .clicked()
        {
            actions.push(Action::BeginShelfRename(slot));
        }
        if chrome::icon(ui, "×")
            .on_hover_text(format!("delete folder ({noun}s spill out)"))
            .clicked()
        {
            actions.push(Action::ScuttleShelf(slot));
        }
        match shelf_edit {
            Some(edit) if edit.shelf == slot => {
                let entry = ui.text_edit_singleline(&mut edit.name);
                if edit.focus {
                    entry.request_focus();
                    edit.focus = false;
                }
                let enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
                if entry.lost_focus() || (entry.has_focus() && enter) {
                    actions.push(Action::CommitShelfRename);
                }
            }
            _ => {
                let _name = ui.label(chrome::section_title(format!(
                    "{} ({})",
                    shelf.name,
                    shelf.entries.len()
                )));
            }
        }
    });
    if header.response.dnd_hover_payload::<EntryName>().is_some() {
        let _glow = ui.painter().rect_stroke(
            header.response.rect,
            2.0,
            egui::Stroke::new(1.0_f32, chrome::HOT),
            egui::StrokeKind::Inside,
        );
    }
    if let Some(payload) = header.response.dnd_release_payload::<EntryName>() {
        actions.push(Action::Moor {
            name: (*payload).clone(),
            berth: Berth::Shelf(slot),
        });
    }
    if shelf.open {
        let _body = ui.indent(id.with("body"), |ui| {
            if shelf.entries.is_empty() {
                let _empty = ui.label(chrome::muted("empty"));
            }
            for entry in &shelf.entries {
                entry_row(ui, noun, active, entry, actions);
            }
        });
    }
}

fn root_basin<T>(ui: &mut egui::Ui, actions: &mut Vec<Action<T>>) {
    if !egui::DragAndDrop::has_any_payload(ui.ctx()) {
        return;
    }
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 14.0), egui::Sense::hover());
    let hot = response.dnd_hover_payload::<EntryName>().is_some();
    let stroke = egui::Stroke::new(1.0_f32, if hot { chrome::HOT } else { chrome::EDGE });
    let _line = ui.painter().hline(rect.x_range(), rect.center().y, stroke);
    if let Some(payload) = response.dnd_release_payload::<EntryName>() {
        actions.push(Action::Moor {
            name: (*payload).clone(),
            berth: Berth::Root,
        });
    }
}
