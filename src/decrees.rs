use std::sync::OnceLock;

use eternalist_apps::commands::{
    CommandCanon, CommandScope, CommandSpec, Shortcut, ShortcutKey, ShortcutModifiers,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Decree {
    FollowLatest,
    FollowLatestLong,
    UndoMapChange,
    ResetConus,
    ToggleCloseToTray,
}

const LATEST: [Shortcut; 1] = [Shortcut::primary('R')];
const LATEST_LONG: [Shortcut; 1] = [Shortcut::new(
    ShortcutModifiers::PRIMARY.plus(ShortcutModifiers::SHIFT),
    ShortcutKey::Character('R'),
)];
const UNDO: [Shortcut; 1] = [Shortcut::primary('Z')];

const DECREES: [CommandSpec<Decree, ()>; 5] = [
    CommandSpec::new(
        Decree::FollowLatest,
        "forecast.follow_latest",
        "Latest run",
        CommandScope::Global,
    )
    .with_detail("Follows the newest ordinary HRRR cycle as it advances.")
    .with_default_shortcuts(&LATEST)
    .with_mnemonic('L'),
    CommandSpec::new(
        Decree::FollowLatestLong,
        "forecast.follow_latest_long",
        "Latest long run",
        CommandScope::Global,
    )
    .with_detail("Follows the newest cycle carrying the extended forecast horizon.")
    .with_default_shortcuts(&LATEST_LONG)
    .with_mnemonic('G'),
    CommandSpec::new(
        Decree::UndoMapChange,
        "map.undo_change",
        "Undo map change",
        CommandScope::Global,
    )
    .with_detail("Restores the last probe or pin arrangement for the active view.")
    .with_default_shortcuts(&UNDO),
    CommandSpec::new(
        Decree::ResetConus,
        "map.reset_conus",
        "Reset CONUS",
        CommandScope::Global,
    )
    .with_detail("Restores the continental United States overview.")
    .with_mnemonic('C'),
    CommandSpec::new(
        Decree::ToggleCloseToTray,
        "application.toggle_close_to_tray",
        "Close to tray",
        CommandScope::Global,
    )
    .with_detail("Hides HRRR on close when the window system and tray support it.")
    .with_mnemonic('T'),
];

pub fn canon() -> &'static CommandCanon<Decree, ()> {
    static CANON: OnceLock<CommandCanon<Decree, ()>> = OnceLock::new();
    CANON.get_or_init(|| CommandCanon::new(&DECREES))
}
