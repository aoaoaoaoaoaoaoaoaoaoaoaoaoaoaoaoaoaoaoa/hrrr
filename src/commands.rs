use std::sync::OnceLock;

use eternalist_apps::commands::{
    CommandCanon, CommandScope, CommandSpec, Shortcut, ShortcutKey, ShortcutModifiers,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Edict {
    FollowLatest,
    FollowLatestLong,
    UndoMapChange,
}

const LATEST: [Shortcut; 1] = [Shortcut::primary('R')];
const LATEST_LONG: [Shortcut; 1] = [Shortcut::new(
    ShortcutModifiers::PRIMARY.plus(ShortcutModifiers::SHIFT),
    ShortcutKey::Character('R'),
)];
const UNDO: [Shortcut; 1] = [Shortcut::primary('Z')];

const EDICTS: [CommandSpec<Edict, ()>; 3] = [
    CommandSpec::new(
        Edict::FollowLatest,
        "forecast.follow_latest",
        "Latest run",
        CommandScope::Global,
    )
    .with_detail("Follow each new HRRR run.")
    .with_default_shortcuts(&LATEST)
    .with_mnemonic('L'),
    CommandSpec::new(
        Edict::FollowLatestLong,
        "forecast.follow_latest_long",
        "Latest long run",
        CommandScope::Global,
    )
    .with_detail("Follow each new 48-hour run.")
    .with_default_shortcuts(&LATEST_LONG)
    .with_mnemonic('G'),
    CommandSpec::new(
        Edict::UndoMapChange,
        "map.undo_change",
        "Undo map change",
        CommandScope::Global,
    )
    .with_detail("Undo the last pin or probe change.")
    .with_default_shortcuts(&UNDO),
];

pub fn canon() -> &'static CommandCanon<Edict, ()> {
    static CANON: OnceLock<CommandCanon<Edict, ()>> = OnceLock::new();
    CANON.get_or_init(|| CommandCanon::new(&EDICTS))
}
