use std::sync::OnceLock;

use eternalist_apps::commands::{
    CommandCanon, CommandScope, CommandSpec, Shortcut, ShortcutKey, ShortcutModifiers,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Decree {
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

const DECREES: [CommandSpec<Decree, ()>; 3] = [
    CommandSpec::new(
        Decree::FollowLatest,
        "forecast.follow_latest",
        "Latest run",
        CommandScope::Global,
    )
    .with_detail("Follow each new HRRR run.")
    .with_default_shortcuts(&LATEST)
    .with_mnemonic('L'),
    CommandSpec::new(
        Decree::FollowLatestLong,
        "forecast.follow_latest_long",
        "Latest long run",
        CommandScope::Global,
    )
    .with_detail("Follow each new 48-hour run.")
    .with_default_shortcuts(&LATEST_LONG)
    .with_mnemonic('G'),
    CommandSpec::new(
        Decree::UndoMapChange,
        "map.undo_change",
        "Undo map change",
        CommandScope::Global,
    )
    .with_detail("Undo the last pin or probe change.")
    .with_default_shortcuts(&UNDO),
];

pub fn canon() -> &'static CommandCanon<Decree, ()> {
    static CANON: OnceLock<CommandCanon<Decree, ()>> = OnceLock::new();
    CANON.get_or_init(|| CommandCanon::new(&DECREES))
}
