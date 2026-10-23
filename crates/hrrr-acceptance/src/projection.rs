//! HRRR's desktop projection of its score: every act through published
//! targets, native X11 input, and witnessed conditions. Coordinates appear
//! only where geometry is the act itself, derived from witnessed anchors.

use std::time::Duration;

use egui_tester::{
    ActionReceipt, Anchor, Application, ArtifactStem, Button, Choreography, Condition, Drag, Error,
    Key, Modifiers, Motion, ProbeFrame, Result, Story, StoryObserver, StoryTempo, Wheel,
};
use egui_tester_witness::encode_identity;
use hrrr_choreography::{
    Act, Direction, Field, HrrrProjection, Modal, Panel, RunTether, ViewAct, Zoom,
};
use hrrr_contract::Target;

use crate::observation::{Dock, Observation, near, shows};

const APPLICATION_NAME: &str = "eternalist.application.name";
const HELP: &str = "eternalist.application.help";
const SETTINGS_OPEN: &str = "eternalist.settings.open";
const SETTINGS_CLOSE: &str = "eternalist.settings.close";
const DRAWER_NEXT: &str = "eternalist.drawer.next";
const DRAWER_PREVIOUS: &str = "eternalist.drawer.previous";
const DRAWER_LOWER: &str = "eternalist.drawer.lower";
const DRAWER_RAISE: &str = "eternalist.drawer.raise";
const VIEWS: &str = "views";
const FOCUS: Duration = Duration::from_secs(5);
/// Live forecast and basemap bytes may cross the network before a frame paints.
const PAINT: Duration = Duration::from_mins(2);
/// A deliberate hand: slow enough to read on film, fast enough for acceptance.
const HAND: Drag = Drag {
    button: Button::Primary,
    press_duration: Duration::from_millis(60),
    steps: 24,
    duration: Duration::from_millis(480),
};
const GLIDE: Motion = Motion {
    steps: 24,
    duration: Duration::from_millis(420),
};
const NOTCH: Wheel = Wheel {
    tick_duration: Duration::from_millis(40),
};
/// Map-relative strike points: probe, its second home, and the pin.
const PROBE: [f32; 2] = [0.72, 0.30];
const PROBE_MOVED: [f32; 2] = [0.30, 0.62];
const PIN: [f32; 2] = [0.44, 0.38];

/// Whether an act also awaits the painted surface after its own witness.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fence {
    /// Synchronize on the act's witnessed consequence alone.
    Witness,
    /// Also await the active field, its latched scale, and the required
    /// basemap on the surface: the specimen world's presentation law.
    Paint,
}

/// One HRRR launch's desktop projection over a borrowed story.
pub struct Desk<'story, 'app, 'bed, O> {
    story: &'story mut Story<'app, 'bed, Observation, O>,
    app: &'app Application<'bed>,
    fence: Fence,
}

/// Enact one act with witness synchronization alone, for an acceptance story.
pub fn enact<'app, 'bed, O: StoryObserver>(
    story: &mut Story<'app, 'bed, Observation, O>,
    app: &'app Application<'bed>,
    act: Act,
) -> Result<()> {
    Desk::new(story, app, Fence::Witness).enact(act)
}

impl<'story, 'app, 'bed, O: StoryObserver> Desk<'story, 'app, 'bed, O> {
    pub const fn new(
        story: &'story mut Story<'app, 'bed, Observation, O>,
        app: &'app Application<'bed>,
        fence: Fence,
    ) -> Self {
        Self { story, app, fence }
    }

    fn state(&mut self) -> Result<Observation> {
        Ok(self.story.frame()?.state)
    }

    fn await_frame(
        &mut self,
        description: &str,
        predicate: impl FnMut(&ProbeFrame<Observation>) -> bool,
    ) -> Result<()> {
        self.story
            .probe_mut()
            .wait(self.app, FOCUS, description, predicate)
            .map(drop)
    }

    fn await_focus(&mut self, target: &str) -> Result<()> {
        self.story
            .probe_mut()
            .wait_focus(self.app, target, FOCUS)
            .map(drop)
    }

    /// Await the active field, its latched scale, the required basemap, and
    /// every plaque on the surface.
    pub fn painted(&mut self) -> Result<()> {
        self.story
            .wait_within(
                PAINT,
                Condition::new(
                    "active field, latched scale, and required basemap painted",
                    |state: &Observation| state.poise.painted,
                ),
            )
            .map(drop)
    }

    fn tap(
        &mut self,
        target: impl std::fmt::Display,
        condition: Condition<Observation>,
    ) -> Result<()> {
        self.story
            .tap(target, Button::Primary, GLIDE)?
            .until(condition)
            .map(drop)
    }

    /// Carry ring focus to `panel`: the first Control+Tab chord leans the
    /// act's way, then each step takes the shorter way around the ring.
    fn seek_panel(&mut self, panel: Panel, lean: Modifiers) -> Result<()> {
        if self.state()?.poise.drawer.is_some() {
            return self.page_drawer(panel);
        }
        if panel == Panel::Application {
            return self.head();
        }
        let target = panel_target(panel);
        let mut step = lean;
        let span = ring().count();
        for _ in 0..=span {
            let Some(focused) = focused_panel(&self.story.frame()?) else {
                self.step_ring(step)?;
                continue;
            };
            if focused == panel {
                break;
            }
            let ahead = (ring_slot(panel) + span - ring_slot(focused)) % span;
            step = if ahead <= span / 2 {
                Modifiers::CTRL
            } else {
                Modifiers::CTRL | Modifiers::SHIFT
            };
            self.step_ring(step)?;
        }
        self.await_focus(&target)
    }

    /// Move ring focus one panel header: Control+Tab advances from the active
    /// panel, Control+Shift+Tab retreats.
    fn step_ring(&mut self, modifiers: Modifiers) -> Result<()> {
        let prior = focused_panel(&self.story.frame()?);
        let _pressed = self.story.chord(modifiers, Key::Tab)?.next_frame()?;
        self.await_frame("Inspector focus to reach another panel header", |frame| {
            focused_panel(frame).is_some_and(|panel| Some(panel) != prior)
        })
    }

    /// Leave the panel ring for the application header above it.
    fn head(&mut self) -> Result<()> {
        let _clicked = self.story.click(APPLICATION_NAME)?.next_frame()?;
        self.await_frame("panel focus to yield to the application header", |frame| {
            focused_panel(frame).is_none()
        })
    }

    /// Page the handheld Drawer, one arrow at a time, to `panel`.
    fn page_drawer(&mut self, panel: Panel) -> Result<()> {
        let wire = panel.wire();
        for _ in Panel::ALL {
            let shown = self.drawer_at_rest()?.panel;
            let at = Panel::ALL
                .into_iter()
                .find(|candidate| candidate.wire() == shown)
                .ok_or_else(|| verdict(format!("Drawer shows unknown panel `{shown}`")))?;
            if at == panel {
                return Ok(());
            }
            let arrow = if panel.index() > at.index() {
                DRAWER_NEXT
            } else {
                DRAWER_PREVIOUS
            };
            self.tap(
                arrow,
                Condition::new(
                    format!("Drawer paged from {shown} toward {wire}"),
                    move |state: &Observation| {
                        state
                            .poise
                            .drawer
                            .as_ref()
                            .is_some_and(|drawer| drawer.settled && drawer.panel != shown)
                    },
                ),
            )?;
        }
        Err(verdict(format!("Drawer never reached the {wire} panel")))
    }

    fn drawer_at_rest(&mut self) -> Result<crate::observation::Drawer> {
        self.story
            .wait(Condition::new("Drawer at rest", |state: &Observation| {
                state
                    .poise
                    .drawer
                    .as_ref()
                    .is_some_and(|drawer| drawer.settled)
            }))?
            .state
            .poise
            .drawer
            .ok_or_else(|| verdict("Drawer vanished at rest"))
    }

    fn raise(&mut self, raised: bool) -> Result<()> {
        let poise = self.state()?.poise;
        match (poise.dock, poise.drawer) {
            (Some(dock), _) => self.dock(dock, raised),
            (None, Some(_)) => self.drawer(raised),
            (None, None) => Err(verdict("HRRR presents neither a Dock nor a Drawer")),
        }
    }

    /// F9 slides the Dock; its witnessed rest ends the act.
    fn dock(&mut self, dock: Dock, raised: bool) -> Result<()> {
        let rest = Dock {
            raised,
            resting: true,
        };
        if dock == rest {
            return Ok(());
        }
        let condition = Condition::new(
            if raised {
                "Dock raised and at rest"
            } else {
                "Dock lowered and at rest"
            },
            move |state: &Observation| state.poise.dock == Some(rest),
        );
        if dock.raised == raised {
            return self.story.wait(condition).map(drop);
        }
        self.story.key(Key::Function(9))?.until(condition).map(drop)
    }

    /// The Drawer's arrows move only while it rests; a rectangle read
    /// mid-slide would send the click to the map beneath.
    fn drawer(&mut self, raised: bool) -> Result<()> {
        if self.drawer_at_rest()?.raised == raised {
            return Ok(());
        }
        self.tap(
            if raised { DRAWER_RAISE } else { DRAWER_LOWER },
            Condition::new(
                if raised {
                    "Drawer raised and at rest"
                } else {
                    "Drawer lowered and at rest"
                },
                move |state: &Observation| {
                    state
                        .poise
                        .drawer
                        .as_ref()
                        .is_some_and(|drawer| drawer.settled && drawer.raised == raised)
                },
            ),
        )
    }

    fn open_modal(&mut self, modal: Modal) -> Result<()> {
        let (target, condition) = match modal {
            Modal::Settings => (
                SETTINGS_OPEN,
                Condition::new("Settings Sheet open", |state: &Observation| {
                    state.settings.open
                }),
            ),
            Modal::CommandGuide => (
                HELP,
                Condition::new("Command Guide open", |state: &Observation| state.guide_open),
            ),
        };
        if self.story.frame()?.anchor(target).is_some() {
            return self.tap(target, condition);
        }
        match modal {
            Modal::Settings => self.story.chord(Modifiers::CTRL, Key::Character(','))?,
            Modal::CommandGuide => self.story.key(Key::Function(1))?,
        }
        .until(condition)
        .map(drop)
    }

    fn close_modal(&mut self, modal: Modal) -> Result<()> {
        match modal {
            Modal::Settings => self.tap(
                SETTINGS_CLOSE,
                Condition::new("Settings Sheet closed", |state: &Observation| {
                    !state.settings.open
                }),
            ),
            Modal::CommandGuide => self
                .story
                .key(Key::Escape)?
                .until(Condition::new(
                    "Command Guide closed",
                    |state: &Observation| !state.guide_open,
                ))
                .map(drop),
        }
    }

    fn pan_map(&mut self) -> Result<()> {
        let map = self.story.anchor(Target::Map)?;
        let before = self.state()?.viewport.center;
        self.story
            .drag_from(within(&map, [0.5, 0.5]), within(&map, [0.36, 0.40]), HAND)?
            .until(Condition::new(
                "map panned and at rest",
                move |state: &Observation| {
                    !state.poise.navigating && !near(state.viewport.center, before)
                },
            ))
            .map(drop)
    }

    fn zoom_map(&mut self, zoom: Zoom) -> Result<()> {
        let map = self.story.anchor(Target::Map)?;
        let before = self.state()?.viewport.zoom;
        let (ticks, description) = match zoom {
            Zoom::In => (-4, "map zoomed in and at rest"),
            Zoom::Out => (4, "map zoomed out and at rest"),
        };
        self.story
            .wheel(map.center(), ticks, NOTCH)?
            .until(Condition::new(description, move |state: &Observation| {
                !state.poise.navigating
                    && match zoom {
                        Zoom::In => state.viewport.zoom > before,
                        Zoom::Out => state.viewport.zoom < before,
                    }
            }))
            .map(drop)
    }

    fn strike_probe(&mut self, point: [f32; 2]) -> Result<()> {
        let map = self.story.anchor(Target::Map)?;
        let before = self.state()?.transient_probe;
        self.story
            .click_at(within(&map, point), Button::Primary)?
            .until(Condition::new(
                "transient probe struck",
                move |state: &Observation| {
                    state
                        .transient_probe
                        .is_some_and(|probe| before.is_none_or(|prior| !near(probe, prior)))
                },
            ))
            .map(drop)
    }

    fn place_pin(&mut self) -> Result<()> {
        let map = self.story.anchor(Target::Map)?;
        let count = self.state()?.pins.len();
        self.story
            .modified_click_at(within(&map, PIN), Button::Primary, Modifiers::SHIFT)?
            .until(shows::pins(count + 1))
            .map(drop)
    }

    fn drag_pin(&mut self) -> Result<()> {
        let slot = last_pin(&self.state()?)?;
        let from = self.story.anchor(Target::Pin(slot))?.center();
        let press = self
            .story
            .session()
            .button_down(from.0, from.1, Button::Primary)?;
        let tugged = self.tug(press, slot, from);
        if tugged.is_err() {
            let _released = self.story.session().button_up(Button::Primary);
        }
        tugged
    }

    /// Carry a seized pin, witnessing capture mid-motion before release.
    fn tug(&mut self, press: ActionReceipt, slot: usize, from: (i16, i16)) -> Result<()> {
        let origin = self
            .story
            .reaction(press)
            .next_frame()?
            .into_value()
            .state
            .pins[slot];
        let to = (from.0.saturating_add(96), from.1.saturating_sub(48));
        let moved = self
            .story
            .motion_to(to, GLIDE)?
            .until(
                shows::dragging(Some(slot))
                    & Condition::new(format!("pin {slot} carried"), move |state: &Observation| {
                        state
                            .pins
                            .get(slot)
                            .is_some_and(|point| !near(*point, origin))
                    }),
            )?
            .into_value();
        let displaced = moved.state.pins[slot];
        let release = self.story.session().button_up(Button::Primary)?;
        self.story
            .reaction(release)
            .until(shows::dragging(None) & shows::pin(slot, displaced))
            .map(drop)
    }

    fn remove_pin(&mut self) -> Result<()> {
        let state = self.state()?;
        let slot = last_pin(&state)?;
        self.tap(Target::PinRemove(slot), shows::pins(state.pins.len() - 1))
    }

    fn undo_map_change(&mut self) -> Result<()> {
        let before = self.state()?;
        let (pins, probe) = (before.pins, before.transient_probe);
        self.story
            .chord(Modifiers::CTRL, Key::Character('z'))?
            .until(Condition::new(
                "pin or probe change undone",
                move |state: &Observation| {
                    state.pins.len() != pins.len()
                        || state
                            .pins
                            .iter()
                            .zip(&pins)
                            .any(|(now, then)| !near(*now, *then))
                        || match (state.transient_probe, probe) {
                            (Some(now), Some(then)) => !near(now, then),
                            (now, then) => now.is_some() != then.is_some(),
                        }
                },
            ))
            .map(drop)
    }

    fn select_field(&mut self, field: Field) -> Result<()> {
        if self.state()?.active_field.as_deref() == Some(field.wire()) {
            return Ok(());
        }
        self.tap(Target::Field(field), shows::field(field))
    }

    fn step_forecast_hour(&mut self, direction: Direction) -> Result<()> {
        let before = self.state()?.lead_hour;
        let target = match direction {
            Direction::Previous => Target::ValidHourEarlier,
            Direction::Next => Target::ValidHourLater,
        };
        self.tap(
            target,
            Condition::new(
                "valid hour stepped",
                move |state: &Observation| match direction {
                    Direction::Previous => state.lead_hour < before,
                    Direction::Next => state.lead_hour > before,
                },
            ),
        )
    }

    /// Seize a Rail, carry it to its origin, then halfway out: the hour it
    /// governs must settle on its floor, then follow the hand past it.
    fn scrub(&mut self, target: Target, hour: fn(&Observation) -> Option<u8>) -> Result<()> {
        let floor = match target {
            Target::ForecastHour => self.state()?.base_hour.map_or(0, |base| base + 1),
            _ => 0,
        };
        let rail = self.story.anchor(target)?;
        let grip = within(&rail, [0.5, 0.5]);
        let press = self
            .story
            .session()
            .button_down(grip.0, grip.1, Button::Primary)?;
        let carried = self.carry(press, &rail, target, hour, floor);
        let release = self.story.session().button_up(Button::Primary)?;
        carried?;
        self.story.reaction(release).next_frame().map(drop)
    }

    fn carry(
        &mut self,
        press: ActionReceipt,
        rail: &Anchor,
        target: Target,
        hour: fn(&Observation) -> Option<u8>,
        floor: u8,
    ) -> Result<()> {
        let _seized = self.story.reaction(press).next_frame()?;
        let _floored = self
            .story
            .motion_to(within(rail, [0.01, 0.5]), GLIDE)?
            .until(Condition::new(
                format!("{target} at its floor"),
                move |state: &Observation| hour(state) == Some(floor),
            ))?;
        self.story
            .motion_to(within(rail, [0.5, 0.5]), GLIDE)?
            .until(Condition::new(
                format!("{target} to follow the hand past its floor"),
                move |state: &Observation| hour(state).is_some_and(|hour| hour > floor),
            ))
            .map(drop)
    }

    fn follow_run(&mut self, tether: RunTether) -> Result<()> {
        let (modifiers, wire) = match tether {
            RunTether::Latest => (Modifiers::CTRL, "latest"),
            RunTether::LatestLong => (Modifiers::CTRL | Modifiers::SHIFT, "latest-long"),
        };
        self.story
            .chord(modifiers, Key::Character('r'))?
            .until(Condition::new(
                format!("forecast tethered to the {wire} run"),
                move |state: &Observation| state.run.as_ref().is_some_and(|run| run.tether == wire),
            ))
            .map(drop)
    }

    fn step_run(&mut self, direction: Direction) -> Result<()> {
        let before = self
            .state()?
            .run
            .and_then(|run| run.cycle)
            .ok_or_else(|| verdict("no forecast cycle to step from"))?;
        let target = match direction {
            Direction::Previous => Target::RunOlder,
            Direction::Next => Target::RunNewer,
        };
        self.tap(
            target,
            Condition::new("fixed run stepped", move |state: &Observation| {
                state.run.as_ref().is_some_and(|run| {
                    run.tether == "fixed"
                        && run.cycle.as_ref().is_some_and(|cycle| match direction {
                            Direction::Previous => *cycle < before,
                            Direction::Next => *cycle > before,
                        })
                })
            }),
        )
    }

    fn manage_views(&mut self, act: ViewAct) -> Result<()> {
        let state = self.state()?;
        match act {
            ViewAct::RenameActive => {
                let renamed = format!("{}-trace", state.active_view);
                let condition = Condition::new(format!("active view renamed to {renamed}"), {
                    let renamed = renamed.clone();
                    move |state: &Observation| state.active_view == renamed
                });
                self.rename(
                    &Target::ViewRename.to_string(),
                    &Target::ViewName.to_string(),
                    &renamed,
                    condition,
                )
            }
            ViewAct::Create => {
                let before = state.active_view;
                self.tap(
                    Target::ViewCreate,
                    Condition::new("new view active", move |state: &Observation| {
                        state.active_view != before && state.holds_view(&state.active_view)
                    }),
                )
            }
            ViewAct::Clone => {
                let view = rooted(&state, None)?;
                let count = state.view_count();
                self.tap(
                    cabinet("clone", &view)?,
                    Condition::new(format!("{view} cloned"), move |state: &Observation| {
                        state.view_count() == count + 1
                    }),
                )
            }
            ViewAct::RenameEntry => {
                let view = rooted(&state, None)?;
                let renamed = format!("{view}-trace");
                let condition = Condition::new(format!("{view} renamed to {renamed}"), {
                    let (view, renamed) = (view.clone(), renamed.clone());
                    move |state: &Observation| {
                        state.holds_view(&renamed) && !state.holds_view(&view)
                    }
                });
                self.rename(
                    &cabinet("rename", &view)?,
                    &cabinet("entry-edit", &view)?,
                    &renamed,
                    condition,
                )
            }
            ViewAct::Load => {
                let view = rooted(&state, Some(state.active_view.as_str()))?;
                let condition = Condition::new(format!("{view} loaded"), {
                    let view = view.clone();
                    move |state: &Observation| state.active_view == view
                });
                self.tap(cabinet("entry", &view)?, condition)
            }
            ViewAct::DragEntryToFolder => {
                let view = rooted(&state, None)?;
                let folder = folder(&state, 0)?;
                let condition = Condition::new(format!("{view} shelved in {folder}"), {
                    let view = view.clone();
                    move |state: &Observation| {
                        state
                            .cabinet
                            .folders
                            .first()
                            .is_some_and(|folder| folder.views.contains(&view))
                    }
                });
                self.carry_entry(
                    &cabinet("entry-drag", &view)?,
                    &cabinet("shelf", &folder)?,
                    condition,
                )
            }
            ViewAct::DragEntryToRoot => {
                let view = state
                    .cabinet
                    .folders
                    .iter()
                    .find_map(|folder| folder.views.first().cloned())
                    .ok_or_else(|| verdict("no shelved view to return to the root"))?;
                let condition = Condition::new(format!("{view} returned to the root"), {
                    let view = view.clone();
                    move |state: &Observation| state.cabinet.views.contains(&view)
                });
                self.carry_entry(
                    &cabinet("entry-drag", &view)?,
                    &format!("cabinet.{VIEWS}.root-berth"),
                    condition,
                )
            }
            ViewAct::DragFolder => {
                let order = state
                    .cabinet
                    .folders
                    .iter()
                    .map(|folder| folder.name.clone())
                    .collect::<Vec<_>>();
                let (Some(first), Some(last)) = (order.first(), order.last()) else {
                    return Err(verdict("no folders to reorder"));
                };
                // A folder lands after its berth when released below the
                // berth's midline; aim above it.
                let berth = self.story.anchor(cabinet("shelf-drag", first)?)?;
                self.story
                    .drag(
                        cabinet("shelf-drag", last)?,
                        within(&berth, [0.5, 0.2]),
                        HAND,
                    )?
                    .until(Condition::new(
                        "folders reordered",
                        move |state: &Observation| {
                            state
                                .cabinet
                                .folders
                                .iter()
                                .map(|folder| &folder.name)
                                .ne(order.iter())
                        },
                    ))
                    .map(drop)
            }
            ViewAct::DeleteEntry => {
                let view = state
                    .cabinet
                    .views
                    .iter()
                    .rev()
                    .find(|view| **view != state.active_view)
                    .cloned()
                    .ok_or_else(|| verdict("no disposable saved view"))?;
                let condition = Condition::new(format!("{view} deleted"), {
                    let view = view.clone();
                    move |state: &Observation| !state.holds_view(&view)
                });
                self.tap(cabinet("delete", &view)?, condition)
            }
            ViewAct::CreateFolder => {
                let count = state.cabinet.folders.len();
                self.tap(
                    format!("cabinet.{VIEWS}.new-shelf"),
                    Condition::new("folder created", move |state: &Observation| {
                        state.cabinet.folders.len() == count + 1
                    }),
                )
            }
            ViewAct::RenameFolder => {
                let folder = folder(&state, 0)?;
                let renamed = format!("{folder}-trace");
                let condition = Condition::new(format!("{folder} renamed to {renamed}"), {
                    let renamed = renamed.clone();
                    move |state: &Observation| {
                        state
                            .cabinet
                            .folders
                            .iter()
                            .any(|folder| folder.name == renamed)
                    }
                });
                self.rename(
                    &cabinet("shelf-rename", &folder)?,
                    &cabinet("shelf-edit", &folder)?,
                    &renamed,
                    condition,
                )
            }
            ViewAct::ToggleFolder => {
                let folder = folder(&state, 0)?;
                let open = state.cabinet.folders[0].open;
                self.tap(
                    cabinet("shelf", &folder)?,
                    Condition::new(format!("{folder} folded"), move |state: &Observation| {
                        state
                            .cabinet
                            .folders
                            .first()
                            .is_some_and(|folder| folder.open != open)
                    }),
                )
            }
            ViewAct::DeleteFolder => {
                let count = state.cabinet.folders.len();
                let folder = folder(&state, count.saturating_sub(1))?;
                self.tap(
                    cabinet("shelf-delete", &folder)?,
                    Condition::new(format!("{folder} deleted"), move |state: &Observation| {
                        state.cabinet.folders.len() + 1 == count
                    }),
                )
            }
        }
    }

    /// Open a name editor, replace its text as a person would, and commit.
    fn rename(
        &mut self,
        actuator: &str,
        editor: &str,
        name: &str,
        renamed: Condition<Observation>,
    ) -> Result<()> {
        let _opened = self
            .story
            .tap(actuator, Button::Primary, GLIDE)?
            .next_frame()?;
        self.await_focus(editor)?;
        let _typed = self.story.replace_focused_text(name)?.next_frame()?;
        self.story.key(Key::Return)?.until(renamed).map(drop)
    }

    fn carry_entry(
        &mut self,
        source: &str,
        destination: &str,
        condition: Condition<Observation>,
    ) -> Result<()> {
        self.story
            .drag_to(source, destination, HAND)?
            .until(condition)
            .map(drop)
    }
}

impl<O: StoryObserver> Choreography for Desk<'_, '_, '_, O> {
    fn chapter(&mut self, title: &str) -> Result<()> {
        self.story.chapter(title)
    }

    fn hold(&mut self, duration: Duration) -> Result<()> {
        self.story.hold(duration)
    }

    fn tempo(&mut self, tempo: StoryTempo) -> Result<()> {
        self.story.tempo(tempo)
    }

    /// A still publishes only a painted surface the pointer has left.
    fn still(&mut self, name: &ArtifactStem) -> Result<()> {
        let _left = self.story.leave()?;
        self.painted()?;
        self.story.still(name)
    }
}

impl<O: StoryObserver> HrrrProjection for Desk<'_, '_, '_, O> {
    fn enact(&mut self, act: Act) -> Result<()> {
        match act {
            Act::SelectPanel(panel) => self.seek_panel(panel, Modifiers::CTRL),
            Act::LowerInspector => self.raise(false),
            Act::RaiseInspector => self.raise(true),
            Act::NextPanel(panel) => self.seek_panel(panel, Modifiers::CTRL),
            Act::PreviousPanel(panel) => self.seek_panel(panel, Modifiers::CTRL | Modifiers::SHIFT),
            Act::OpenModal(modal) => self.open_modal(modal),
            Act::CloseModal(modal) => self.close_modal(modal),
            Act::PanMap => self.pan_map(),
            Act::ZoomMap(zoom) => self.zoom_map(zoom),
            Act::PlaceProbe => self.strike_probe(PROBE),
            Act::MoveProbe => self.strike_probe(PROBE_MOVED),
            Act::ClearProbe => self
                .story
                .key(Key::Escape)?
                .until(Condition::new(
                    "transient probe cleared",
                    |state: &Observation| state.transient_probe.is_none(),
                ))
                .map(drop),
            Act::PlacePin => self.place_pin(),
            Act::DragPin => self.drag_pin(),
            Act::RemovePin => self.remove_pin(),
            Act::UndoMapChange => self.undo_map_change(),
            Act::SelectField(field) => self.select_field(field),
            Act::StepForecastHour(direction) => self.step_forecast_hour(direction),
            Act::DragForecastHour => {
                self.scrub(Target::ForecastHour, |state| Some(state.lead_hour))
            }
            Act::DragBaseHour => self.scrub(Target::BaseHour, |state| state.base_hour),
            Act::FollowRun(tether) => self.follow_run(tether),
            Act::StepRun(direction) => self.step_run(direction),
            Act::ManageViews(act) => self.manage_views(act),
        }?;
        match self.fence {
            Fence::Witness => Ok(()),
            Fence::Paint => self.painted(),
        }
    }
}

fn panel_target(panel: Panel) -> String {
    Target::Panel(panel).to_string()
}

/// The panels the Dock's Control+Tab ring visits, in presentation order;
/// the application header stands above the ring.
fn ring() -> impl Iterator<Item = Panel> {
    Panel::ALL
        .into_iter()
        .filter(|panel| *panel != Panel::Application)
}

fn ring_slot(panel: Panel) -> usize {
    panel.index() - Panel::Field.index()
}

fn focused_panel(frame: &ProbeFrame<Observation>) -> Option<Panel> {
    ring().find(|panel| frame.focused_anchor(&panel_target(*panel)).is_some())
}

/// A point at fractional coordinates of a witnessed rectangle.
fn within(anchor: &Anchor, [x, y]: [f32; 2]) -> (i16, i16) {
    let [x0, y0, x1, y1] = anchor.rect;
    (
        (x0 + (x1 - x0) * x).round() as i16,
        (y0 + (y1 - y0) * y).round() as i16,
    )
}

fn last_pin(state: &Observation) -> Result<usize> {
    state
        .pins
        .len()
        .checked_sub(1)
        .ok_or_else(|| verdict("no persistent pin on the map"))
}

/// The first root view other than `except`.
fn rooted(state: &Observation, except: Option<&str>) -> Result<String> {
    state
        .cabinet
        .views
        .iter()
        .find(|view| Some(view.as_str()) != except)
        .cloned()
        .ok_or_else(|| verdict("no saved view at the cabinet root"))
}

fn folder(state: &Observation, slot: usize) -> Result<String> {
    state
        .cabinet
        .folders
        .get(slot)
        .map(|folder| folder.name.clone())
        .ok_or_else(|| verdict(format!("no view folder in slot {slot}")))
}

/// One views-cabinet target, its identity encoded as the witness wire demands.
fn cabinet(part: &str, identity: &str) -> Result<String> {
    let identity = encode_identity(identity)
        .ok_or_else(|| verdict(format!("cabinet {part} target has an empty identity")))?;
    Ok(format!("cabinet.{VIEWS}.{part}/{identity}"))
}

fn verdict(detail: impl Into<String>) -> Error {
    Error::Verdict {
        detail: detail.into(),
    }
}
