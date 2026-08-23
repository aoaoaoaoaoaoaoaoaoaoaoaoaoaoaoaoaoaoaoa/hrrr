use std::time::Duration;

use egui_tester::{Button, Condition, Key, Modifiers, Motion, PixelRegion, Probe, Result, demand};

use crate::{harness::Harness, observation::Observation};

const WAIT: Duration = Duration::from_secs(5);

pub fn run(harness: &Harness<'_>) -> Result<()> {
    let app = harness.launch(true)?;
    let mut focus: Probe<Observation> = app.witness()?.typed();
    let mut story = harness.story(&app)?;
    let initial = story.wait(Condition::new(
        "command guide closed",
        |state: &Observation| !state.guide_open,
    ))?;
    let initial_close = initial.state.close_to_tray;

    let name = story.anchor("eternalist.application.name")?.rect;
    let help = story.anchor("eternalist.application.help")?.rect;
    let settings = story.anchor("eternalist.settings.open")?.rect;
    let first_panel = story.anchor(hrrr_contract::Target::Panel("field"))?.rect;
    demand(
        name[0] < help[0]
            && help[0] < settings[0]
            && name[1] <= help[3]
            && help[1] <= name[3]
            && settings[3] < first_panel[1],
        "application header did not present NAME, Help, Settings above the control panels",
    )?;

    let before = story.capture()?;
    let _opened = story.key(Key::Function(1))?.until(Condition::new(
        "command guide open",
        |state: &Observation| state.guide_open,
    ))?;
    let guide = focus.wait_anchor(&app, "eternalist.command-guide.body", WAIT)?;
    let _presented_after_guide = focus.wait_fresh(&app, WAIT)?;
    let _compositor_margin = focus.wait_fresh(&app, WAIT)?;
    let guide_region = PixelRegion::anchor(&guide);
    let visible = story
        .session()
        .wait_changed_region(&before, guide_region, 0.55, 2, WAIT)?;
    demand(
        before.difference_region(&visible, guide_region, 2)? > 0.55,
        "F1 changed the witness without presenting the generated command guide",
    )?;
    if let Some(artifacts) = harness.artifacts {
        before.save_png(artifacts.join("hrrr-before-command-guide.png"))?;
        visible.save_png(artifacts.join("hrrr-command-guide.png"))?;
    }

    let blocked = story
        .chord(Modifiers::ALT, Key::Character('t'))?
        .next_frame()?
        .into_value();
    demand(
        blocked.state.guide_open && blocked.state.close_to_tray == initial_close,
        "Alt+T escaped through the open command guide",
    )?;
    let _closed = story.key(Key::Escape)?.until(Condition::new(
        "command guide closed",
        |state: &Observation| !state.guide_open,
    ))?;
    let _current = focus.read()?;
    let _modal_retired = focus.wait_fresh(&app, WAIT)?;
    let _focus_restored = focus.wait_fresh(&app, WAIT)?;

    let _settings = story.key(Key::Function(2))?.until(Condition::new(
        "F2 settings sheet open",
        |state: &Observation| state.settings.open && !state.settings.fault,
    ))?;
    let blocked = story
        .chord(Modifiers::ALT, Key::Character('t'))?
        .next_frame()?
        .into_value();
    demand(
        blocked.state.settings.open && blocked.state.close_to_tray == initial_close,
        "Alt+T escaped through the open settings sheet",
    )?;
    let toggled = !initial_close;
    let _toggled = story
        .tap(
            "eternalist.settings.close_to_tray",
            Button::Primary,
            Motion::default(),
        )?
        .until(Condition::new(
            "central close-to-tray setting applied",
            move |state: &Observation| state.close_to_tray == toggled,
        ))?;
    let _closed = story
        .chord(Modifiers::CTRL, Key::Character(','))?
        .until(Condition::new(
            "settings sheet closed",
            |state: &Observation| !state.settings.open,
        ))?;

    let _next_panel = story.chord(Modifiers::CTRL, Key::Tab)?.next_frame()?;
    let _forecast = focus.wait_focus(
        &app,
        &hrrr_contract::Target::Panel("forecast").to_string(),
        WAIT,
    )?;
    let _previous_panel = story
        .chord(Modifiers::CTRL | Modifiers::SHIFT, Key::Tab)?
        .next_frame()?;
    let _field = focus.wait_focus(
        &app,
        &hrrr_contract::Target::Panel("field").to_string(),
        WAIT,
    )?;

    let _persisted = story.wait_stable(
        Duration::from_secs(3),
        Duration::from_millis(700),
        "close-to-tray setting persisted",
        move |frame| (frame.state.close_to_tray == toggled).then_some(()),
    )?;
    app.terminate()?;
    drop(story);
    drop(app);

    let app = harness.launch(true)?;
    let mut story = harness.story(&app)?;
    let _restored = story.wait(Condition::new(
        "close-to-tray mnemonic survived restart",
        move |state: &Observation| state.close_to_tray == toggled,
    ))?;
    app.terminate()
}
