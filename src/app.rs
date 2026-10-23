use crate::view::ViewSlot;
use crate::{
    application_paths::{ApplicationPaths, InstanceGuard},
    basemap::{self, Basemap, TileKey, VectorTile},
    cache::Custodian,
    commands::{self, Edict},
    configuration::Configuration,
    library::EntryName,
    library_ui::{self, Action as ViewAction, EntryEdit, NameEdit, ShelfEdit},
    map,
    model::{
        FieldGrid, ForecastRun, ForecastSystem, FrameKey, LeadHour, MercatorPoint, Product, RunId,
        RunSelection, Viewport,
    },
    spec::{RangeRegime, Scale, ScaleAtlas, TemperatureSeason},
    state::SessionState,
    vector_map::VectorPaint,
    view::{SavedView, ViewLibrary},
    wind_barb,
    worker::{Command, DemandId, Event, LoadDemand, LoadIntent, Worker},
};
use anyhow::Result;
use brass_poolrooms::{
    chrome,
    water::{Domain, Frame as WaterFrame, Surface, Wetness},
};
use egui::Color32;
use eternalist_apps::{
    ApplicationHeader, Capabilities, Drawer, ScribeOutcome, SettledScribe,
    command_guide::{CommandGuide, GuideGesture, GuideGroup},
    commands::{CommandDispatch, CommandStatus, Shortcut, ShortcutKey, ShortcutModifiers},
    configuration::ConfigurationLedger,
    panel_navigation::PanelNavigator,
    responsiveness::DrainBudget,
    settings::{SettingSpec, SettingsFile, SettingsSheet},
};
use hrrr_contract::{Panel, Target};

mod canvas;

use canvas::PinTug;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};

const STATE_SETTLE: Duration = Duration::from_millis(450);
const SCALE_SETTLE: Duration = Duration::from_millis(180);
const FRONTIER_POLL: Duration = Duration::from_mins(1);
const TILE_RETRY_DELAY: Duration = Duration::from_secs(15);
const EVENT_DRAIN: DrainBudget = DrainBudget::new(64, Duration::from_millis(3));
const FIELD_CAPACITY: usize = 12;
/// A tap this close to the transient probe clears it instead of moving it.
const TRANSIENT_PROBE_REAP_RADIUS: f32 = 18.0;
/// Field buttons and run steps grow to a fingertip under touch.
const POINTER_BUTTON_HEIGHT: f32 = 26.0;
const TOUCH_BUTTON_HEIGHT: f32 = 44.0;
/// Panel titles, in carousel order.
const DRAWER_ROSTER: [&str; Panel::ALL.len()] = {
    let mut roster = [""; Panel::ALL.len()];
    let mut slot = 0;
    while slot < roster.len() {
        roster[slot] = title(Panel::ALL[slot]);
        slot += 1;
    }
    roster
};
const VECTOR_CEILING: usize = 512 * 1_048_576;
const CLOSE_TO_TRAY: SettingSpec = SettingSpec::new(
    "close_to_tray",
    "CLOSE TO TRAY",
    "Closing the window hides HRRR instead of ending the forecast session.",
);
const VIEW_SLOT_KEYS: [Shortcut; 10] = [
    Shortcut::new(ShortcutModifiers::NONE, ShortcutKey::Character('0')),
    Shortcut::new(ShortcutModifiers::NONE, ShortcutKey::Character('1')),
    Shortcut::new(ShortcutModifiers::NONE, ShortcutKey::Character('2')),
    Shortcut::new(ShortcutModifiers::NONE, ShortcutKey::Character('3')),
    Shortcut::new(ShortcutModifiers::NONE, ShortcutKey::Character('4')),
    Shortcut::new(ShortcutModifiers::NONE, ShortcutKey::Character('5')),
    Shortcut::new(ShortcutModifiers::NONE, ShortcutKey::Character('6')),
    Shortcut::new(ShortcutModifiers::NONE, ShortcutKey::Character('7')),
    Shortcut::new(ShortcutModifiers::NONE, ShortcutKey::Character('8')),
    Shortcut::new(ShortcutModifiers::NONE, ShortcutKey::Character('9')),
];
const ASSIGN_VIEW_SLOT_KEYS: [Shortcut; 10] = [
    Shortcut::new(ShortcutModifiers::SHIFT, ShortcutKey::Character('0')),
    Shortcut::new(ShortcutModifiers::SHIFT, ShortcutKey::Character('1')),
    Shortcut::new(ShortcutModifiers::SHIFT, ShortcutKey::Character('2')),
    Shortcut::new(ShortcutModifiers::SHIFT, ShortcutKey::Character('3')),
    Shortcut::new(ShortcutModifiers::SHIFT, ShortcutKey::Character('4')),
    Shortcut::new(ShortcutModifiers::SHIFT, ShortcutKey::Character('5')),
    Shortcut::new(ShortcutModifiers::SHIFT, ShortcutKey::Character('6')),
    Shortcut::new(ShortcutModifiers::SHIFT, ShortcutKey::Character('7')),
    Shortcut::new(ShortcutModifiers::SHIFT, ShortcutKey::Character('8')),
    Shortcut::new(ShortcutModifiers::SHIFT, ShortcutKey::Character('9')),
];
const TOGGLE_CONTROLS: [Shortcut; 1] = [Shortcut::new(
    ShortcutModifiers::NONE,
    ShortcutKey::Function(9),
)];
const NEXT_CONTROL_SECTION: [Shortcut; 1] =
    [Shortcut::new(ShortcutModifiers::CONTROL, ShortcutKey::Tab)];
const PREVIOUS_CONTROL_SECTION: [Shortcut; 1] = [Shortcut::new(
    ShortcutModifiers::CONTROL.plus(ShortcutModifiers::SHIFT),
    ShortcutKey::Tab,
)];
const ADJUST_FORECAST_TIME: [Shortcut; 2] = [
    Shortcut::new(ShortcutModifiers::NONE, ShortcutKey::ArrowLeft),
    Shortcut::new(ShortcutModifiers::NONE, ShortcutKey::ArrowRight),
];
const FORECAST_TIME_BOUNDS: [Shortcut; 2] = [
    Shortcut::new(ShortcutModifiers::NONE, ShortcutKey::Home),
    Shortcut::new(ShortcutModifiers::NONE, ShortcutKey::End),
];
const FORECAST_CONTROL_GESTURES: [GuideGesture; 5] = [
    GuideGesture::new(
        "Toggle controls",
        "Show or hide the forecast controls.",
        &TOGGLE_CONTROLS,
    ),
    GuideGesture::new(
        "Next control section",
        "Focus the next forecast group.",
        &NEXT_CONTROL_SECTION,
    ),
    GuideGesture::new(
        "Previous control section",
        "Focus the previous forecast group.",
        &PREVIOUS_CONTROL_SECTION,
    ),
    GuideGesture::new(
        "Adjust forecast time",
        "Step the focused forecast hour; the wheel also works while hovering.",
        &ADJUST_FORECAST_TIME,
    ),
    GuideGesture::new(
        "First or last forecast time",
        "Jump to the first or last forecast hour.",
        &FORECAST_TIME_BOUNDS,
    ),
];
const VIEW_GESTURES: [GuideGesture; 2] = [
    GuideGesture::new(
        "Select bound view",
        "Open the view bound to a number.",
        &VIEW_SLOT_KEYS,
    ),
    GuideGesture::new(
        "Bind active view",
        "Bind the current view to a number.",
        &ASSIGN_VIEW_SLOT_KEYS,
    ),
];
const FORECAST_CONTROL_GUIDE_GROUP: GuideGroup =
    GuideGroup::new("FORECAST CONTROLS", &FORECAST_CONTROL_GESTURES);
const VIEW_GUIDE_GROUP: GuideGroup = GuideGroup::new("VIEW SHORTCUTS", &VIEW_GESTURES);
const INSPECTOR_TOUCH_GESTURES: [GuideGesture; 3] = [
    GuideGesture::new(
        "Switch panel",
        "Moves to the adjacent Inspector panel.",
        &[],
    )
    .cued("SWIPE ↔"),
    GuideGesture::new(
        "Scroll panel",
        "Moves vertically through the current panel.",
        &[],
    )
    .cued("SWIPE ↕"),
    GuideGesture::new(
        "Lower or raise Inspector",
        "Leaves the map exposed or restores the current panel.",
        &[],
    )
    .cued("TAP CENTER ARROW"),
];
const MAP_TOUCH_GESTURES: [GuideGesture; 5] = [
    GuideGesture::new("Pan map", "Moves the map beneath one finger.", &[]).cued("DRAG"),
    GuideGesture::new(
        "Zoom map",
        "Changes scale around the gesture's center.",
        &[],
    )
    .cued("PINCH"),
    GuideGesture::new(
        "Probe forecast",
        "Places or moves a temporary reading.",
        &[],
    )
    .cued("TAP"),
    GuideGesture::new("Clear probe", "Removes the temporary reading.", &[]).cued("TAP PROBE"),
    GuideGesture::new(
        "Keep or move pin",
        "Touch and hold the map to keep a reading; drag its forged head to move it.",
        &[],
    )
    .cued("HOLD / DRAG PIN"),
];
const VIEW_TOUCH_GESTURES: [GuideGesture; 1] = [GuideGesture::new(
    "Rearrange views",
    "Moves a view or folder to the indicated berth.",
    &[],
)
.cued("DRAG HANDLE")];
const INSPECTOR_TOUCH_GROUP: GuideGroup = GuideGroup::new("INSPECTOR", &INSPECTOR_TOUCH_GESTURES);
const MAP_TOUCH_GROUP: GuideGroup = GuideGroup::new("MAP", &MAP_TOUCH_GESTURES);
const VIEW_TOUCH_GROUP: GuideGroup = GuideGroup::new("VIEWS", &VIEW_TOUCH_GESTURES);
const GUIDE_GROUPS: [GuideGroup; 5] = [
    FORECAST_CONTROL_GUIDE_GROUP,
    VIEW_GUIDE_GROUP,
    INSPECTOR_TOUCH_GROUP,
    MAP_TOUCH_GROUP,
    VIEW_TOUCH_GROUP,
];

#[derive(Clone, Copy)]
enum TileRejection {
    Absent,
    RetryAt(Instant),
}

impl TileRejection {
    fn blocks(self, now: Instant) -> bool {
        match self {
            Self::Absent => true,
            Self::RetryAt(deadline) => now < deadline,
        }
    }

    const fn resolves(self) -> bool {
        matches!(self, Self::Absent)
    }
}

#[derive(Clone, Copy, PartialEq)]
struct RangeScene {
    key: FrameKey,
    viewport: Viewport,
    extent: [f32; 2],
}

#[derive(Default)]
struct RangeSurvey {
    product: Option<Product>,
    scene: Option<RangeScene>,
    peak: Option<f32>,
}

struct ScaleLatch<S> {
    settled: S,
    contender: Option<(S, Instant)>,
}

impl<S: Default> Default for ScaleLatch<S> {
    fn default() -> Self {
        Self {
            settled: S::default(),
            contender: None,
        }
    }
}

impl<S: Copy + Eq> ScaleLatch<S> {
    const fn settled(&self) -> S {
        self.settled
    }

    fn observe(&mut self, proposed: S, now: Instant) -> Option<Duration> {
        if proposed == self.settled {
            self.contender = None;
            return None;
        }
        let Some((contender, born)) = self.contender else {
            self.contender = Some((proposed, now));
            return Some(SCALE_SETTLE);
        };
        if contender != proposed {
            self.contender = Some((proposed, now));
            return Some(SCALE_SETTLE);
        }
        let age = now.saturating_duration_since(born);
        if age >= SCALE_SETTLE {
            self.settled = proposed;
            self.contender = None;
            None
        } else {
            Some(SCALE_SETTLE.saturating_sub(age))
        }
    }

    fn arrest(&mut self) {
        self.contender = None;
    }

    #[cfg(feature = "egui-test")]
    const fn latched(&self) -> bool {
        self.contender.is_none()
    }

    fn reset(&mut self)
    where
        S: Default,
    {
        *self = Self::default();
    }
}

enum MapReversal {
    Pins {
        view: EntryName,
        before: Vec<MercatorPoint>,
    },
    Probe {
        before: Option<MercatorPoint>,
    },
}

impl MapReversal {
    fn belongs_to(&self, view: &EntryName) -> bool {
        match self {
            Self::Pins { view: owner, .. } => owner == view,
            Self::Probe { .. } => true,
        }
    }

    fn recoil(self, pins: &[MercatorPoint], probe: Option<MercatorPoint>) -> Option<MapRecoil> {
        match self {
            Self::Pins { before, .. } if before != pins => Some(MapRecoil::Pins(before)),
            Self::Probe { before } if before != probe => Some(MapRecoil::Probe(before)),
            Self::Pins { .. } | Self::Probe { .. } => None,
        }
    }
}

#[derive(Debug, PartialEq)]
enum MapRecoil {
    Pins(Vec<MercatorPoint>),
    Probe(Option<MercatorPoint>),
}

#[derive(Default)]
struct MapUndo {
    reversals: VecDeque<MapReversal>,
}

impl MapUndo {
    const CAPACITY: usize = 64;

    fn remember(&mut self, reversal: MapReversal) {
        if self.reversals.len() == Self::CAPACITY {
            let _forgotten = self.reversals.pop_front();
        }
        self.reversals.push_back(reversal);
    }

    fn has_reversal_for(&self, view: &EntryName) -> bool {
        self.reversals
            .iter()
            .any(|reversal| reversal.belongs_to(view))
    }

    fn recoil(
        &mut self,
        view: &EntryName,
        pins: &[MercatorPoint],
        probe: Option<MercatorPoint>,
    ) -> Option<MapRecoil> {
        loop {
            let slot = self
                .reversals
                .iter()
                .rposition(|reversal| reversal.belongs_to(view))?;
            let reversal = self.reversals.remove(slot)?;
            if let Some(recoil) = reversal.recoil(pins, probe) {
                return Some(recoil);
            }
        }
    }

    fn rename_view(&mut self, old: &EntryName, new: &EntryName) {
        for reversal in &mut self.reversals {
            if let MapReversal::Pins { view, .. } = reversal
                && view == old
            {
                view.clone_from(new);
            }
        }
    }

    fn forget_view(&mut self, view: &EntryName) {
        self.reversals.retain(
            |reversal| !matches!(reversal, MapReversal::Pins { view: owner, .. } if owner == view),
        );
    }
}

impl RangeSurvey {
    fn select(&mut self, product: Product) -> bool {
        if self.product == Some(product) {
            return false;
        }
        self.product = Some(product);
        self.scene = None;
        self.peak = None;
        true
    }

    fn discern(
        &mut self,
        key: FrameKey,
        field: &FieldGrid,
        viewport: Viewport,
        rect: egui::Rect,
    ) -> Option<f32> {
        let scene = RangeScene {
            key,
            viewport,
            extent: [rect.width(), rect.height()],
        };
        if self.scene != Some(scene) {
            self.scene = Some(scene);
            self.peak = map::visible_peak(field, viewport, rect);
        }
        self.peak
    }
}

/// How the Inspector is disposed: the Dock needs a hovering pointer for its
/// actuator; without one the Drawer rises from the bottom edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Disposition {
    Dock,
    Drawer,
}

impl Disposition {
    const fn for_capabilities(capabilities: Capabilities) -> Self {
        if capabilities.pointer {
            Self::Dock
        } else {
            Self::Drawer
        }
    }
}

#[derive(Clone, Copy, Default)]
struct DirtyState {
    session_state: bool,
    views: bool,
}

impl DirtyState {
    const fn any(self) -> bool {
        self.session_state || self.views
    }
}

struct DurableState {
    session_state: Option<SessionState>,
    views: Option<ViewLibrary>,
}

impl DurableState {
    fn save(self, paths: &ApplicationPaths) -> Result<()> {
        let mut faults = Vec::new();
        if let Some(session_state) = self.session_state
            && let Err(error) = session_state.save(&paths.session_state_path())
        {
            faults.push(format!("session state: {error:#}"));
        }
        if let Some(views) = self.views
            && let Err(error) = views.save(&paths.views_path())
        {
            faults.push(format!("saved views: {error:#}"));
        }
        if faults.is_empty() {
            Ok(())
        } else {
            Err(anyhow::anyhow!(faults.join("; ")))
        }
    }
}

pub struct WeatherApp {
    _instance: InstanceGuard,
    capabilities: Capabilities,
    disposition: Disposition,
    map_multitouch: bool,
    configuration: ConfigurationLedger<Configuration>,
    settings: SettingsSheet,
    views: ViewLibrary,
    session_state: SessionState,
    active_view: EntryName,
    viewport: Viewport,
    pins: Vec<MercatorPoint>,
    run: Option<ForecastRun>,
    worker: Worker,
    basemap: Basemap,
    custodian: Custodian,
    latest_runs: HashMap<ForecastSystem, RunId>,
    run_extents: HashMap<ForecastRun, LeadHour>,
    surveying_run: Option<ForecastRun>,
    next_survey: Instant,
    announced_discovery: bool,
    demand_id: DemandId,
    loading: Option<LoadDemand>,
    fields: FrameBank,
    displayed_field: Option<(FrameKey, Arc<FieldGrid>)>,
    prefetch: VecDeque<FrameKey>,
    tiles: VectorBank,
    presented_basemap: Arc<[Arc<VectorTile>]>,
    tile_inflight: HashSet<TileKey>,
    tile_rejections: HashMap<TileKey, TileRejection>,
    panels: PanelNavigator,
    guide: CommandGuide,
    transient_probe: Option<MercatorPoint>,
    pin_tug: Option<PinTug>,
    map_undo: MapUndo,
    view_name_entry: String,
    name_edit: NameEdit,
    shelf_edit: Option<ShelfEdit>,
    entry_edit: Option<EntryEdit>,
    scales: ScaleAtlas,
    range_scale: ScaleLatch<RangeRegime>,
    range_survey: RangeSurvey,
    scale_bar: map::ScaleBar,
    water: Surface,
    scribe: SettledScribe<DurableState>,
    dirty: DirtyState,
    status: String,
    basemap_status: String,
    #[cfg(feature = "egui-test")]
    poise: crate::witness::Poise,
}

impl WeatherApp {
    pub fn open_at(
        ctx: &egui::Context,
        paths: ApplicationPaths,
        instance: InstanceGuard,
    ) -> Result<Self> {
        let capabilities = Capabilities::of(ctx);
        let legacy_views = Configuration::migrate_legacy_views(&paths.config_path())?;
        let configuration: ConfigurationLedger<Configuration> = ConfigurationLedger::raise(
            "hrrr-configuration-scribe",
            ctx,
            paths.config_path(),
            STATE_SETTLE,
        )?;

        chrome::set_font_scale(ctx, configuration.live().font_scale);
        let mut settings = SettingsSheet::default();
        if configuration.fault().is_some() {
            settings.require_attention(ctx);
        }
        let (mut views, migrated_views) = ViewLibrary::load(&paths.views_path(), legacy_views)?;
        let (mut session_state, migrated_slate) = SessionState::load(&paths.session_state_path())?;
        views.restore_folds(&session_state.closed_folders);
        let active_view = views
            .active(session_state.active_view.clone())
            .or_else(|| views.all().next().map(|view| view.name.clone()));
        let Some(active_view) = active_view else {
            anyhow::bail!("view library admitted no active document");
        };
        let Some(view) = views.get(&active_view) else {
            anyhow::bail!("active view `{active_view}` vanished during startup");
        };
        let viewport = view.viewport;
        let pins = view.pins.clone();
        let run = session_state
            .cycle
            .fixed()
            .zip(session_state.overlay.active())
            .map(|(run, product)| ForecastRun::forge(product.system(), run));
        session_state.active_view = Some(active_view.clone());
        let worker = Worker::spawn(ctx.clone(), &paths)?;
        let basemap = Basemap::spawn(ctx.clone(), &paths)?;
        let custodian = Custodian::spawn(ctx.clone(), paths.cache_manager())?;
        #[cfg(feature = "egui-test")]
        let run_extents = witnessed_frontier(run).into_iter().collect();
        #[cfg(not(feature = "egui-test"))]
        let run_extents = HashMap::new();
        let wetness = if capabilities.power_unconstrained {
            Wetness::Wet
        } else {
            Wetness::Dry
        };
        let mut water = Surface::new(wetness);
        {
            let (chemistry, agitation) = water.laboratory_mut();
            chemistry.refract_px = 0.34;
            chemistry.meniscus_px = 0.62;
            chemistry.ior_spread = 0.14;
            chemistry.bulge_px = 3.2;
            chemistry.source_gain = 18.0;
            agitation.enter_impulse = 0.22;
            agitation.exit_impulse = 0.12;
            agitation.click_impulse = 0.62;
            agitation.scroll_coupling = 0.006;
            agitation.pond_impulse = 0.4;
        }
        let scribe_lair = paths.clone();
        let mut scribe = SettledScribe::spawn(
            "hrrr-state-scribe",
            ctx,
            STATE_SETTLE,
            move |state: DurableState| state.save(&scribe_lair),
        )?;
        let dirty = DirtyState {
            session_state: migrated_slate,
            views: migrated_views,
        };
        if dirty.any() {
            scribe.mark();
        }
        let app = Self {
            _instance: instance,
            capabilities,
            disposition: Disposition::for_capabilities(capabilities),
            map_multitouch: false,
            configuration,
            settings,
            views,
            session_state,
            active_view,
            viewport,
            pins,
            run,
            worker,
            basemap,
            custodian,
            latest_runs: HashMap::new(),
            run_extents,
            surveying_run: None,
            next_survey: Instant::now(),
            announced_discovery: true,
            demand_id: DemandId::default(),
            loading: None,
            fields: FrameBank::new(FIELD_CAPACITY),
            displayed_field: None,
            prefetch: VecDeque::new(),
            tiles: VectorBank::new(VECTOR_CEILING),
            presented_basemap: Arc::from([]),
            tile_inflight: HashSet::new(),
            tile_rejections: HashMap::new(),
            panels: PanelNavigator::default(),
            guide: CommandGuide::default(),
            transient_probe: None,
            pin_tug: None,
            map_undo: MapUndo::default(),
            view_name_entry: String::new(),
            name_edit: NameEdit::Idle,
            shelf_edit: None,
            entry_edit: None,
            scales: ScaleAtlas::default(),
            range_scale: ScaleLatch::default(),
            range_survey: RangeSurvey::default(),
            scale_bar: map::ScaleBar::default(),
            water,
            scribe,
            dirty,
            status: "finding latest forecast…".to_owned(),
            basemap_status: "OPENING MAP".to_owned(),
            #[cfg(feature = "egui-test")]
            poise: crate::witness::Poise::default(),
        };
        app.worker.send(Command::Discover)?;
        Ok(app)
    }

    pub fn pulse(&mut self, ui: &mut egui::Ui) {
        #[cfg(feature = "egui-test")]
        {
            self.poise = crate::witness::Poise::default();
        }
        chrome::set_font_scale(ui.ctx(), self.configuration.live().font_scale);
        self.absorb_events(ui.ctx());
        self.absorb_persistence();
        let _configuration_changed = self.configuration.absorb();
        if self.configuration.fault().is_some() {
            self.settings.require_attention(ui.ctx());
        }
        let settings_invoked = self.settings.take_shortcut(ui.ctx());
        if settings_invoked && self.settings.is_open() {
            self.reload_configuration();
        }
        let guide_invoked = self.guide.take_shortcuts(ui.ctx());
        if !settings_invoked
            && !guide_invoked
            && let Some(dispatch) =
                commands::canon().route(ui.ctx(), &[], |edict| self.edict_status(edict))
        {
            self.apply_edict(dispatch);
        }
        self.take_keys(ui.ctx());
        match self.disposition {
            Disposition::Dock => self.dock(ui),
            Disposition::Drawer => self.drawer(ui),
        }
        let _center = egui::CentralPanel::default().show(ui, |ui| self.map(ui));
        let mut guide = std::mem::take(&mut self.guide);
        guide.show(
            ui.ctx(),
            commands::canon(),
            &[],
            |()| "FORECAST",
            |edict| self.edict_status(edict),
            &GUIDE_GROUPS,
        );
        self.guide = guide;
        self.show_settings(ui.ctx());
    }

    fn dock(&mut self, ui: &mut egui::Ui) {
        let mut panels = std::mem::take(&mut self.panels);
        let inspector = eternalist_apps::Inspector::new("forecast-inspector")
            .scroll_id("forecast-inspector-scroll")
            .scroll_offset(self.session_state.inspector_scroll)
            .show(ui, |ui| self.inspector(ui, &mut panels));
        self.panels = panels;
        if inspector.scroll_offset != self.session_state.inspector_scroll {
            self.session_state.inspector_scroll = inspector.scroll_offset;
            self.mark_dirty();
        }
        #[cfg(feature = "egui-test")]
        {
            let berth = if inspector.is_expanded() {
                eternalist_apps::inspector::WIDTH
            } else {
                0.0
            };
            self.poise.dock = Some(crate::witness::Dock {
                raised: inspector.is_expanded(),
                resting: (inspector.visible_extent() - berth).abs() < 0.01,
            });
        }
        inspector.agitate(&mut self.water);
    }

    fn drawer(&mut self, ui: &mut egui::Ui) {
        let initial = if self.session_state.overlay.active().is_some() {
            Panel::Forecast
        } else {
            self.session_state.drawer_panel.unwrap_or(Panel::Field)
        };
        let response = Drawer::new("forecast-inspector-drawer", &DRAWER_ROSTER)
            .initial_panel(initial as usize)
            .show(ui, |ui, panel| {
                self.drawer_panel(ui, Panel::ALL[panel]);
            });
        self.water.begin(Domain::basin(response.domain));
        for control in &response.controls {
            self.water.monoglyph(control);
        }
        let shown = Panel::ALL[response.panel];
        #[cfg(feature = "egui-test")]
        {
            self.poise.drawer = Some(crate::witness::Drawer {
                raised: response.raised,
                settled: response.settled,
                panel: shown.wire(),
            });
        }
        if self.session_state.drawer_panel != Some(shown) {
            self.session_state.drawer_panel = Some(shown);
            self.mark_dirty();
        }
    }

    fn drawer_panel(&mut self, ui: &mut egui::Ui, panel: Panel) {
        crate::witness::anchor(ui, Target::Panel(panel), ui.available_rect_before_wrap());
        match panel {
            Panel::Application => {
                let _header = ApplicationHeader::new("HRRR")
                    .settings_attention(self.configuration.fault().is_some())
                    .show(ui, &mut self.guide, &mut self.settings, &mut self.water);
                ui.add_space(8.0);
                let _identity = ui.label(chrome::muted(
                    "HIGH-RESOLUTION RAPID REFRESH\nNOAA FORECAST FIELD VIEWER",
                ));
                ui.add_space(8.0);
                let undo = ui
                    .add_enabled_ui(self.map_undo.has_reversal_for(&self.active_view), |ui| {
                        chrome::Monoglyph::symbol(chrome::Symbol::Undo).show(ui)
                    })
                    .inner
                    .on_hover_text("Undo the last pin or probe change");
                self.water.monoglyph(&undo);
                if undo.clicked() {
                    self.undo_map_object();
                }
            }
            Panel::Field => self.field_buttons(ui),
            Panel::Forecast => self.forecast_controls(ui),
            Panel::ActiveView => self.active_view_panel(ui),
            Panel::Views => self.view_library_panel(ui),
            Panel::Status => self.status_panel(ui),
        }
    }

    /// The forecast-field buttons, sized for a pointer or a fingertip.
    fn field_buttons(&mut self, ui: &mut egui::Ui) {
        let height = if self.capabilities.touch {
            TOUCH_BUTTON_HEIGHT
        } else {
            POINTER_BUTTON_HEIGHT
        };
        let mut chosen_product = None;
        for &row in Product::ROWS {
            let _row = ui.horizontal(|ui| {
                let widths = apportion_product_widths(ui, row);
                for (index, &product) in row.iter().enumerate() {
                    let response = ui.add_sized(
                        [widths[index], height],
                        egui::Button::new(product.label())
                            .truncate()
                            .selected(self.session_state.overlay.active() == Some(product)),
                    );
                    crate::witness::anchor(ui, Target::Field(product.field()), response.rect);
                    chrome::tension(ui, &response);
                    if response.hovered() {
                        self.water.hover(("product", product), response.rect);
                    }
                    if response.clicked() {
                        chosen_product = Some((product, response.rect));
                    }
                }
            });
        }
        if let Some((product, rect)) = chosen_product {
            self.water.select(rect);
            self.strike_overlay(product);
        }
    }

    fn status_panel(&self, ui: &mut egui::Ui) {
        let _status = ui.label(chrome::muted(&self.status));
        ui.add_space(3.0);
        let _source = ui.label(chrome::muted(format!(
            "FORECAST · NOAA\nMAP · {}",
            self.basemap_status
        )));
    }

    pub fn service_deadline(&self, _now: Instant) -> Option<Instant> {
        let configuration = self.configuration.deadline();
        self.scribe
            .deadline()
            .into_iter()
            .chain(configuration)
            .chain(self.survey_deadline())
            .chain(self.tile_rejections.values().filter_map(|rejection| {
                if let TileRejection::RetryAt(deadline) = rejection {
                    Some(*deadline)
                } else {
                    None
                }
            }))
            .min()
    }

    pub fn service_deadline_reached(&mut self, now: Instant) -> bool {
        let mut changed = self.configuration.service_deadline_reached(now);
        self.tile_rejections.retain(|_key, rejection| {
            let expired = matches!(rejection, TileRejection::RetryAt(deadline) if *deadline <= now);
            changed |= expired;
            !expired
        });
        if self
            .survey_deadline()
            .is_some_and(|deadline| deadline <= now)
            && let Some(run) = self.run
        {
            self.request_survey(run);
            changed = true;
        }
        if self
            .scribe
            .deadline()
            .is_some_and(|deadline| deadline <= now)
        {
            let snapshot = self.durable_state();
            match self.scribe.tend(now, || snapshot) {
                Ok(Some(_sequence)) => self.dirty = DirtyState::default(),
                Ok(None) => {}
                Err(error) => {
                    self.status = format!("state scribe failed: {error:#}");
                    changed = true;
                }
            }
        }
        changed
    }

    pub fn water_frame(
        &mut self,
        ctx: &egui::Context,
        pixels_per_point: f32,
        tooltip_rects: &[egui::Rect],
    ) -> WaterFrame {
        self.water.frame(ctx, pixels_per_point, tooltip_rects, None)
    }

    /// Checkpoint before the platform takes the surface away.
    pub fn suspend(&mut self) {
        self.retire();
    }

    fn retire(&mut self) {
        if let Err(error) = self.scribe.flush(self.durable_state_all()) {
            eprintln!("could not persist HRRR state during retirement: {error:#}");
        }
        self.dirty = DirtyState::default();
    }

    pub fn close_to_tray_enabled(&self) -> bool {
        self.configuration.live().close_minimizes
    }

    fn reload_configuration(&mut self) {
        if (self.configuration.fault().is_some() || self.configuration.settled())
            && let Err(error) = self.configuration.request_reload()
        {
            self.status = format!("configuration reload failed: {error:#}");
        }
    }

    fn show_settings(&mut self, ctx: &egui::Context) {
        let path = self.configuration.path().to_owned();
        let fault = self.configuration.fault().map(ToString::to_string);
        let mut close_minimizes = self.configuration.live().close_minimizes;
        let mut font_scale = self.configuration.live().font_scale;
        let file = fault.as_deref().map_or_else(
            || SettingsFile::ready(&path),
            |message| SettingsFile::fault(&path, message),
        );
        let file = file
            .reloading(self.configuration.reload_pending())
            .reloadable(self.configuration.fault().is_some() || self.configuration.settled());
        let file = self.capabilities.configuration.then_some(file);
        let tray = !self.capabilities.retirement;
        let mut close_changed = false;
        let mut font_scale_changed = false;
        let response = self.settings.show(ctx, &mut self.water, file, |settings| {
            settings.group("APPEARANCE");
            font_scale_changed |= settings.font_size(&mut font_scale);
            if tray {
                settings.group("WINDOW");
                close_changed |= settings.boolean(CLOSE_TO_TRAY, &mut close_minimizes);
            }
        });
        if close_changed || font_scale_changed {
            match self.configuration.revise(|config| {
                config.close_minimizes = close_minimizes;
                config.font_scale = font_scale;
            }) {
                Ok(_) => {
                    if font_scale_changed {
                        chrome::set_font_scale(ctx, font_scale);
                    }
                    if close_changed {
                        if close_minimizes {
                            "close hides the window"
                        } else {
                            "close quits"
                        }
                        .clone_into(&mut self.status);
                    }
                }
                Err(error) => self.status = format!("configuration change failed: {error:#}"),
            }
        }
        if response.reload_requested() {
            self.reload_configuration();
        }
    }

    fn inspector(&mut self, ui: &mut egui::Ui, navigator: &mut PanelNavigator) {
        let _header = ApplicationHeader::new("HRRR")
            .settings_attention(self.configuration.fault().is_some())
            .show(ui, &mut self.guide, &mut self.settings, &mut self.water);
        ui.add_space(5.0);
        let mut panels = navigator.frame(ui.ctx());
        let field = panels.panel(ui, "product", title(Panel::Field), true, |ui| {
            self.field_buttons(ui);
        });
        crate::witness::response(ui, Target::Panel(Panel::Field), &field.header);
        self.water.fold(field.wake);

        let forecast = panels.panel(ui, "forecast", title(Panel::Forecast), true, |ui| {
            self.forecast_controls(ui);
        });
        crate::witness::response(ui, Target::Panel(Panel::Forecast), &forecast.header);
        self.water.fold(forecast.wake);

        let active_view = panels.panel(ui, "active-view", title(Panel::ActiveView), true, |ui| {
            self.active_view_panel(ui);
        });
        crate::witness::response(ui, Target::Panel(Panel::ActiveView), &active_view.header);
        self.water.fold(active_view.wake);

        let views = panels.panel(ui, "view-library", title(Panel::Views), true, |ui| {
            self.view_library_panel(ui);
        });
        crate::witness::response(ui, Target::Panel(Panel::Views), &views.header);
        self.water.fold(views.wake);

        let status = panels.panel(ui, "status", title(Panel::Status), true, |ui| {
            self.status_panel(ui);
        });
        crate::witness::response(ui, Target::Panel(Panel::Status), &status.header);
        self.water.fold(status.wake);
    }

    fn active_view_panel(&mut self, ui: &mut egui::Ui) {
        let mut edit = self.name_edit;
        let actions = library_ui::active_card(
            ui,
            &mut self.water,
            "view",
            &mut self.view_name_entry,
            &mut edit,
            &self.active_view,
        );
        self.name_edit = edit;
        self.apply_view_actions(ui.ctx(), actions);
    }

    fn view_library_panel(&mut self, ui: &mut egui::Ui) {
        let mut shelf_edit = self.shelf_edit.take();
        let mut entry_edit = self.entry_edit.take();
        let actions = library_ui::library(
            ui,
            &mut self.water,
            "view",
            &self.active_view,
            &self.views,
            &mut shelf_edit,
            &mut entry_edit,
        );
        self.shelf_edit = shelf_edit;
        self.entry_edit = entry_edit;
        self.apply_view_actions(ui.ctx(), actions);
    }

    fn forecast_controls(&mut self, ui: &mut egui::Ui) {
        let Some(run) = self.run else {
            let _waiting = ui.label(chrome::muted("finding latest forecast…"));
            return;
        };
        let run_label = run
            .local_label()
            .unwrap_or_else(|_| "invalid cycle time".to_owned());
        let _run = ui.label(chrome::eyebrow(format!("RUN · {run_label}")));
        ui.add_space(3.0);

        let product = self.session_state.overlay.active();
        let axis = product.and_then(|product| product.lead_axis(run).ok());
        let valid_label = axis
            .and_then(|axis| axis.local_label(self.session_state.lead).ok())
            .or_else(|| run.valid_local_label(self.session_state.lead).ok())
            .unwrap_or_else(|| "invalid valid time".to_owned());
        let horizon = product
            .and_then(|product| product.horizon(run).ok())
            .unwrap_or(LeadHour::ZERO);
        let published = self
            .run_extents
            .get(&run)
            .copied()
            .map(|published| published.min(horizon));
        let cumulative = product.is_some_and(Product::has_baseline);
        let lead_floor = if cumulative {
            self.session_state.base.next()
        } else {
            Some(LeadHour::ZERO)
        };
        let rail_ceiling = axis.map_or(0, |axis| axis.detent_count().saturating_sub(1));
        let ready_ceiling = axis
            .zip(published)
            .and_then(|(axis, frontier)| axis.ready_ceiling(frontier));
        let allowed_floor = axis
            .zip(lead_floor)
            .map_or(0, |(axis, floor)| axis.index_at_or_after(floor));
        let lead_ready = ready_ceiling.is_some_and(|ceiling| allowed_floor <= ceiling);
        let current = axis.map_or(0, |axis| axis.index_at_or_before(self.session_state.lead));
        let mut step = None;
        let _row = ui.horizontal(|ui| {
            let previous = ui
                .add_enabled_ui(lead_ready && current > allowed_floor, |ui| {
                    chrome::Monoglyph::symbol(chrome::Symbol::ArrowLeft).show(ui)
                })
                .inner
                .on_hover_text("Previous valid time");
            crate::witness::response(ui, Target::ValidHourEarlier, &previous);
            self.water.monoglyph(&previous);
            if previous.clicked()
                && let Some(lead) = axis.and_then(|axis| axis.at(current.saturating_sub(1)))
            {
                step = Some((lead, previous.rect));
            }
            let _lead = ui.label(chrome::section_title(&valid_label));
            let next = ui
                .add_enabled_ui(
                    ready_ceiling.is_some_and(|ceiling| lead_ready && current < ceiling),
                    |ui| chrome::Monoglyph::symbol(chrome::Symbol::ArrowRight).show(ui),
                )
                .inner
                .on_hover_text("Next valid time");
            crate::witness::response(ui, Target::ValidHourLater, &next);
            self.water.monoglyph(&next);
            if next.clicked()
                && let Some(lead) = axis.and_then(|axis| axis.at(current.saturating_add(1)))
            {
                step = Some((lead, next.rect));
            }
        });
        if let Some((lead, rect)) = step {
            self.choose_lead(lead);
            self.water.lever(rect, 1.0);
        }
        let rail_detents = rail_ceiling + 1;
        let mut raw_lead = current;
        let rail_allowed = if lead_ready {
            allowed_floor..=ready_ceiling.unwrap_or(allowed_floor)
        } else {
            0..=0
        };
        let rail = ui
            .add_enabled_ui(lead_ready, |ui| {
                chrome::Rail::new(&mut raw_lead, 0..=rail_ceiling)
                    .allowed(rail_allowed)
                    .detents(rail_detents)
                    .show(ui)
            })
            .inner;
        crate::witness::anchor(ui, Target::ForecastHour, rail.rect);
        self.water.rail(&rail);
        if rail.changed()
            && lead_ready
            && let Some(lead) = axis.and_then(|axis| axis.at(raw_lead))
        {
            self.choose_lead(lead);
        }
        if published.is_none() {
            let _surveying = ui.label(chrome::muted("checking available hours…"));
        } else if !lead_ready {
            let _interval = ui.label(chrome::muted("waiting for first accumulation…"));
        }

        if cumulative {
            ui.add_space(4.0);
            let base_label = run
                .valid_local_label(self.session_state.base)
                .unwrap_or_else(|_| "invalid base time".to_owned());
            let _base = ui.horizontal(|ui| {
                let _label = ui.label(chrome::muted("BASE HOUR"));
                let _value = ui.label(chrome::section_title(base_label));
            });
            let base_ceiling = self.session_state.lead.saturating_previous();
            let mut raw_base = u16::from(self.session_state.base.get());
            let base_rail_ceiling = u16::from(horizon.get());
            let base_rail = ui
                .add_enabled_ui(lead_ready, |ui| {
                    chrome::Rail::new(&mut raw_base, 0..=base_rail_ceiling)
                        .allowed(0..=u16::from(base_ceiling.get()))
                        .detents(base_rail_ceiling + 1)
                        .show(ui)
                })
                .inner;
            crate::witness::anchor(ui, Target::BaseHour, base_rail.rect);
            self.water.rail(&base_rail);
            if base_rail.changed()
                && lead_ready
                && let Ok(raw_base) = u8::try_from(raw_base)
                && let Ok(base) = LeadHour::forge(raw_base)
            {
                self.choose_base(base);
            }
        }

        ui.add_space(4.0);
        let latest_run = self.latest_runs.get(&run.system).copied();
        let latest_extended =
            latest_run.map(|latest| RunSelection::LatestLong.bind(run.system, latest));
        let mut run_step = None;
        let latest = ui
            .add_enabled_ui(latest_run.is_some(), |ui| {
                let button =
                    egui::Button::new(commands::canon().spec(Edict::FollowLatest).widget_text(ui))
                        .selected(
                            self.session_state.cycle == RunSelection::Latest
                                && latest_run == Some(run.id),
                        );
                let button = button.shortcut_text(
                    commands::canon().shortcuts(Edict::FollowLatest)[0].label(ui.ctx()),
                );
                ui.add_sized([ui.available_width(), 24.0], button)
            })
            .inner;
        if chrome::exact_activation(ui, &latest) {
            run_step = Some((RunSelection::Latest, latest.rect));
        }
        let latest_long = ui
            .add_enabled_ui(latest_extended.is_some(), |ui| {
                let button = egui::Button::new(
                    commands::canon()
                        .spec(Edict::FollowLatestLong)
                        .widget_text(ui),
                )
                .selected(
                    self.session_state.cycle == RunSelection::LatestLong
                        && latest_extended == Some(run.id),
                );
                let button = button.shortcut_text(
                    commands::canon().shortcuts(Edict::FollowLatestLong)[0].label(ui.ctx()),
                );
                ui.add_sized([ui.available_width(), 24.0], button)
            })
            .inner;
        if chrome::exact_activation(ui, &latest_long) {
            run_step = Some((RunSelection::LatestLong, latest_long.rect));
        }
        let step_height = if self.capabilities.touch {
            TOUCH_BUTTON_HEIGHT
        } else {
            22.0
        };
        let _step = ui.horizontal(|ui| {
            let width = (ui.available_width() - ui.spacing().item_spacing.x) / 2.0;
            let older = ui.add_sized([width, step_height], egui::Button::new("OLDER"));
            crate::witness::response(ui, Target::RunOlder, &older);
            if older.clicked() {
                run_step = run
                    .previous()
                    .ok()
                    .map(|candidate| (RunSelection::Fixed(candidate.id), older.rect));
            }
            let newer = ui
                .add_enabled_ui(latest_run.is_some_and(|latest| run.id < latest), |ui| {
                    ui.add_sized([width, step_height], egui::Button::new("NEWER"))
                })
                .inner;
            crate::witness::response(ui, Target::RunNewer, &newer);
            if newer.clicked() {
                run_step = run.next().ok().and_then(|candidate| {
                    latest_run
                        .map(|latest| (RunSelection::Fixed(candidate.id.min(latest)), newer.rect))
                });
            }
        });
        if let Some((selection, rect)) = run_step {
            self.follow_cycle(selection);
            self.water.click(rect);
        }
    }

    #[cfg(feature = "egui-test")]
    pub fn witness_state(&self) -> crate::witness::State {
        crate::witness::State {
            contract: hrrr_contract::UI_FINGERPRINT,
            launch: "ready",
            active_field: self
                .session_state
                .overlay
                .active()
                .map(|product| product.field().wire()),
            lead_hour: self.session_state.lead.get(),
            base_hour: self
                .session_state
                .overlay
                .active()
                .filter(|product| product.has_baseline())
                .map(|_| self.session_state.base.get()),
            active_view: self.active_view.as_str().to_owned(),
            pins: self.pins.iter().map(|pin| pin.world()).collect(),
            transient_probe: self.transient_probe.map(MercatorPoint::world),
            dragging_pin: self.pin_tug.map(|tug| tug.slot),
            guide_open: self.guide.is_open(),
            close_to_tray: self.configuration.live().close_minimizes,
            settings: crate::witness::Settings {
                open: self.settings.is_open(),
                fault: self.configuration.fault().is_some(),
            },
            viewport: crate::witness::Viewport {
                center: self.viewport.center_mercator,
                zoom: self.viewport.zoom,
            },
            poise: self.poise,
            run: self.run.map(|run| crate::witness::Run {
                cycle: run.id.stamp().ok(),
                tether: match self.session_state.cycle {
                    RunSelection::Latest => "latest",
                    RunSelection::LatestLong => "latest-long",
                    RunSelection::Fixed(_) => "fixed",
                },
            }),
            cabinet: crate::witness::Cabinet {
                views: self
                    .views
                    .saved
                    .iter()
                    .map(|view| eternalist_apps::CabinetEntry::key(view).as_str().to_owned())
                    .collect(),
                folders: self
                    .views
                    .shelves
                    .iter()
                    .map(|shelf| crate::witness::Folder {
                        name: shelf.name.clone(),
                        open: shelf.open,
                        views: shelf
                            .entries
                            .iter()
                            .map(|view| {
                                eternalist_apps::CabinetEntry::key(view).as_str().to_owned()
                            })
                            .collect(),
                    })
                    .collect(),
            },
        }
    }

    fn absorb_events(&mut self, ctx: &egui::Context) {
        let mut drain = EVENT_DRAIN.arm();
        while let Some(message) = drain.receive(&self.custodian.faults) {
            self.status = message;
        }
        while let Some(event) = drain.receive(&self.worker.events) {
            match event {
                Event::Discovered(extent) => {
                    let latest = extent.run();
                    let prior_latest = self.latest_runs.insert(latest.system, latest.id);
                    let _prior_extent = self.run_extents.insert(latest, extent.published());
                    if self
                        .session_state
                        .overlay
                        .active()
                        .is_none_or(|product| product.system() != latest.system)
                    {
                        continue;
                    }
                    let selection = self.session_state.cycle.rectify(latest.id);
                    if selection != self.session_state.cycle {
                        self.session_state.cycle = selection;
                        self.mark_dirty();
                    }
                    let run =
                        ForecastRun::forge(latest.system, selection.bind(latest.system, latest.id));
                    let run_changed = self.run != Some(run);
                    if run_changed {
                        self.rebase_forecast(run);
                        self.mark_dirty();
                    }
                    if self.announced_discovery || prior_latest != Some(latest.id) {
                        "latest forecast found".clone_into(&mut self.status);
                    }
                    self.announced_discovery = false;
                    self.next_survey = Instant::now() + FRONTIER_POLL;
                    if run == latest || self.run_extents.contains_key(&run) {
                        self.reconcile_forecast();
                    } else {
                        self.request_survey(run);
                    }
                }
                Event::Surveyed(extent) => {
                    let run = extent.run();
                    if self.surveying_run == Some(run) {
                        self.surveying_run = None;
                    }
                    let prior = self.run_extents.insert(run, extent.published());
                    self.next_survey = Instant::now() + FRONTIER_POLL;
                    if self.run == Some(run) {
                        self.clamp_clock();
                        let active = self.active_key();
                        if self.session_state.overlay.active().is_some()
                            && (active.is_none()
                                || self
                                    .displayed_field
                                    .as_ref()
                                    .is_none_or(|(key, _field)| Some(*key) != active))
                        {
                            self.demand_active();
                        } else if prior != Some(extent.published()) {
                            "new forecast hours available".clone_into(&mut self.status);
                        }
                    }
                }
                Event::SurveyFault { run, message } => {
                    if self.surveying_run == Some(run) {
                        self.surveying_run = None;
                    }
                    self.next_survey = Instant::now() + FRONTIER_POLL;
                    self.status = message;
                }
                Event::DiscoveryFault { system, message } => {
                    if self
                        .session_state
                        .overlay
                        .active()
                        .is_some_and(|product| product.system() == system)
                    {
                        self.announced_discovery = false;
                        self.status = format!("live refresh failed · {message}");
                    }
                }
                Event::Loaded { demand, field } => {
                    if self.loading == Some(demand) {
                        self.loading = None;
                    }
                    let foreground = demand.intent == LoadIntent::Foreground(self.demand_id)
                        && self.active_key() == Some(demand.key);
                    if foreground {
                        self.displayed_field = Some((demand.key, field.clone()));
                    }
                    self.fields.insert(demand.key, field);
                    if foreground {
                        "forecast ready".clone_into(&mut self.status);
                    }
                    self.kick_prefetch();
                }
                Event::Fault { demand, message } => {
                    if demand.intent == LoadIntent::Foreground(self.demand_id)
                        && self.active_key() == Some(demand.key)
                    {
                        self.status = message;
                    }
                    if self.loading == Some(demand) {
                        self.loading = None;
                    }
                    self.kick_prefetch();
                }
            }
        }
        while let Some(event) = drain.receive(&self.basemap.events) {
            match event {
                basemap::Event::Ready => {
                    "PROTOMAPS · © OPENSTREETMAP CONTRIBUTORS".clone_into(&mut self.basemap_status);
                }
                basemap::Event::Loaded(tile) => {
                    let key = tile.key;
                    let _was_inflight = self.tile_inflight.remove(&key);
                    let _rejection = self.tile_rejections.remove(&key);
                    self.tiles.insert(tile);
                }
                basemap::Event::Missing(key) => {
                    let _was_inflight = self.tile_inflight.remove(&key);
                    let _prior = self.tile_rejections.insert(key, TileRejection::Absent);
                }
                basemap::Event::Fault { key, message } => {
                    let detail = key.is_some_and(TileKey::is_detail);
                    if let Some(key) = key {
                        let _was_inflight = self.tile_inflight.remove(&key);
                        let _prior = self.tile_rejections.insert(
                            key,
                            TileRejection::RetryAt(Instant::now() + TILE_RETRY_DELAY),
                        );
                    }
                    self.basemap_status = if detail {
                        format!("MAP DETAIL OFFLINE · {message}")
                    } else {
                        format!("MAP UNAVAILABLE · {message}")
                    };
                }
            }
        }
        if !self.custodian.faults.is_empty()
            || !self.worker.events.is_empty()
            || !self.basemap.events.is_empty()
        {
            ctx.request_repaint();
        }
    }

    fn survey_deadline(&self) -> Option<Instant> {
        let run = self.run?;
        let incomplete = self
            .run_extents
            .get(&run)
            .is_some_and(|published| run.horizon().is_ok_and(|horizon| *published < horizon));
        (incomplete && self.surveying_run.is_none() && self.loading.is_none())
            .then_some(self.next_survey)
    }

    fn request_survey(&mut self, run: ForecastRun) {
        if self.surveying_run == Some(run) {
            return;
        }
        self.next_survey = Instant::now() + FRONTIER_POLL;
        match self.worker.send(Command::Survey(run)) {
            Ok(()) => {
                self.surveying_run = Some(run);
                "checking available forecast hours…".clone_into(&mut self.status);
            }
            Err(err) => self.status = err.to_string(),
        }
    }

    fn request_discovery(&mut self) {
        match self.worker.send(Command::Discover) {
            Ok(()) => {
                self.announced_discovery = true;
                "finding latest forecast…".clone_into(&mut self.status);
            }
            Err(err) => self.status = err.to_string(),
        }
    }

    fn edict_status(&self, edict: Edict) -> CommandStatus<'static> {
        let system = self.session_state.overlay.active().map(Product::system);
        let latest = system.and_then(|system| self.latest_runs.get(&system).copied());
        match edict {
            Edict::FollowLatest if latest.is_none() => {
                CommandStatus::Disabled("available forecasts are still loading")
            }
            Edict::FollowLatestLong
                if system
                    .zip(latest)
                    .map(|(system, latest)| RunSelection::LatestLong.bind(system, latest))
                    .is_none() =>
            {
                CommandStatus::Disabled("no 48-hour run is available")
            }
            Edict::UndoMapChange if !self.map_undo.has_reversal_for(&self.active_view) => {
                CommandStatus::Disabled("the active view has no map change to undo")
            }
            Edict::FollowLatest | Edict::FollowLatestLong | Edict::UndoMapChange => {
                CommandStatus::Enabled
            }
        }
    }

    fn apply_edict(&mut self, dispatch: CommandDispatch<'_, Edict>) {
        let edict = match dispatch {
            CommandDispatch::Invoke(edict) => edict,
            CommandDispatch::Refused { reason, .. } => {
                self.status = format!("unavailable: {reason}");
                return;
            }
        };
        match edict {
            Edict::FollowLatest => self.follow_cycle(RunSelection::Latest),
            Edict::FollowLatestLong => self.follow_cycle(RunSelection::LatestLong),
            Edict::UndoMapChange => self.undo_map_object(),
        }
    }

    fn take_keys(&mut self, ctx: &egui::Context) {
        if self.guide.is_open() || ctx.memory(|memory| memory.top_modal_layer().is_some()) {
            return;
        }
        let text_edit_focused = ctx.text_edit_focused();
        if !text_edit_focused {
            while let Some((slot, assign)) = consume_view_slot(ctx) {
                if assign {
                    self.assign_view_slot(slot);
                } else {
                    self.load_view_slot(slot);
                }
            }
        }
        if !text_edit_focused
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
            && self.transient_probe.is_some()
        {
            self.remember_probe();
            self.transient_probe = None;
        }
    }

    fn apply_view_actions(&mut self, ctx: &egui::Context, actions: Vec<ViewAction<SavedView>>) {
        if !actions.is_empty() {
            // The cabinet laid this pass out before its actions landed.
            ctx.request_discard("view cabinet changed");
        }
        for action in actions {
            match action {
                ViewAction::New => self.new_view(),
                ViewAction::BeginNameEdit => self.begin_view_name_edit(),
                ViewAction::Rename => self.rename_view(),
                ViewAction::Load(view) => self.load_view(view),
                ViewAction::Clone(name) => self.clone_view(&name),
                ViewAction::Delete(name) => self.delete_view(&name),
                ViewAction::RenameEntry { from, to } => {
                    let _renamed = self.rename_view_to(&from, to);
                }
                ViewAction::Moor { name, berth } => {
                    self.views.moor(&name, &berth);
                    self.mark_views_dirty();
                }
                ViewAction::MoorShelf { shelf, berth } => {
                    self.views.moor_shelf(shelf, berth);
                    self.shelf_edit = None;
                    self.mark_views_dirty();
                }
                ViewAction::NewShelf => {
                    self.views.add_shelf();
                    self.mark_views_dirty();
                }
                ViewAction::ToggleShelf(shelf) => {
                    self.views.toggle_shelf(shelf);
                    self.session_state.closed_folders = self.views.closed_shelves();
                    self.mark_dirty();
                }
                ViewAction::ScuttleShelf(shelf) => {
                    self.views.scuttle_shelf(shelf);
                    self.shelf_edit = None;
                    self.session_state.closed_folders = self.views.closed_shelves();
                    self.mark_dirty();
                    self.mark_views_dirty();
                }
                ViewAction::BeginShelfRename(shelf) => {
                    let name = self
                        .views
                        .shelves
                        .get(shelf)
                        .map(|rack| rack.name.clone())
                        .unwrap_or_default();
                    self.shelf_edit = Some(ShelfEdit {
                        shelf,
                        name,
                        focus: true,
                    });
                }
                ViewAction::CommitShelfRename => {
                    if let Some(edit) = self.shelf_edit.take() {
                        if self.views.rename_shelf(edit.shelf, &edit.name) {
                            self.session_state.closed_folders = self.views.closed_shelves();
                            self.mark_dirty();
                            self.mark_views_dirty();
                        } else {
                            self.status = format!("folder `{}` already exists", edit.name.trim());
                        }
                    }
                }
            }
        }
    }

    fn sync_active_view(&mut self) {
        if let Some(view) = self.views.get_mut(&self.active_view) {
            view.reframe(self.viewport, self.pins.clone());
        } else {
            self.views.upsert(SavedView::forge(
                self.active_view.clone(),
                self.viewport,
                self.pins.clone(),
            ));
        }
        self.session_state.active_view = Some(self.active_view.clone());
        self.mark_dirty();
        self.mark_views_dirty();
    }

    fn remember_pins(&mut self) {
        self.map_undo.remember(MapReversal::Pins {
            view: self.active_view.clone(),
            before: self.pins.clone(),
        });
    }

    fn remember_probe(&mut self) {
        self.map_undo.remember(MapReversal::Probe {
            before: self.transient_probe,
        });
    }

    fn undo_map_object(&mut self) {
        let Some(recoil) =
            self.map_undo
                .recoil(&self.active_view, &self.pins, self.transient_probe)
        else {
            return;
        };
        self.pin_tug = None;
        match recoil {
            MapRecoil::Pins(pins) => {
                self.pins = pins;
                self.sync_active_view();
            }
            MapRecoil::Probe(probe) => self.transient_probe = probe,
        }
    }

    fn new_view(&mut self) {
        self.sync_active_view();
        let source = self.active_view.clone();
        let name = self.views.spare_named(&source);
        let view = SavedView::forge(name.clone(), self.viewport, self.pins.clone());
        self.views.adopt_beside(&source, view);
        self.active_view = name.clone();
        self.session_state.active_view = Some(name.clone());
        self.view_name_entry.clear();
        self.name_edit = NameEdit::Idle;
        self.status = format!("new view `{name}`");
        self.mark_dirty();
        self.mark_views_dirty();
    }

    fn load_view(&mut self, view: SavedView) {
        self.active_view.clone_from(&view.name);
        self.session_state.active_view = Some(view.name.clone());
        self.viewport = view.viewport;
        self.pins = view.pins;
        self.pin_tug = None;
        self.view_name_entry.clear();
        self.name_edit = NameEdit::Idle;
        self.status = format!("active view `{}`", view.name);
        self.mark_dirty();
    }

    fn load_view_slot(&mut self, slot: ViewSlot) {
        let view = self
            .views
            .all()
            .find(|view| view.slot == Some(slot))
            .cloned();
        if let Some(view) = view {
            self.load_view(view);
        } else {
            self.status = format!("view slot {slot} is unbound");
        }
    }

    fn assign_view_slot(&mut self, slot: ViewSlot) {
        self.sync_active_view();
        let mut displaced = None;
        for view in self.views.all_mut() {
            if view.slot == Some(slot) {
                displaced = Some(view.name.clone());
                view.slot = None;
            }
        }
        if let Some(view) = self.views.get_mut(&self.active_view) {
            view.slot = Some(slot);
        }
        self.status = displaced
            .filter(|name| name != &self.active_view)
            .map_or_else(
                || format!("bound view `{}` to {slot}", self.active_view),
                |name| format!("moved slot {slot} from `{name}` to `{}`", self.active_view),
            );
        self.mark_views_dirty();
    }

    fn clone_view(&mut self, name: &EntryName) {
        let Some(source) = self.views.get(name).cloned() else {
            return;
        };
        let clone_name = self.views.spare_named(name);
        let clone = SavedView::forge(clone_name.clone(), source.viewport, source.pins.clone());
        self.views.adopt_beside(name, clone.clone());
        self.load_view(clone);
        self.status = format!("cloned view `{clone_name}`");
        self.mark_views_dirty();
    }

    fn delete_view(&mut self, name: &EntryName) {
        if self.views.all().count() == 1 {
            "the last view cannot be deleted".clone_into(&mut self.status);
            return;
        }
        let Some(removed) = self.views.remove(name) else {
            return;
        };
        self.map_undo.forget_view(&removed.name);
        let successor = (self.active_view == removed.name)
            .then(|| self.views.all().next().cloned())
            .flatten();
        if let Some(next) = successor {
            self.load_view(next);
        }
        self.status = format!("deleted view `{}`", removed.name);
        self.mark_views_dirty();
    }

    fn begin_view_name_edit(&mut self) {
        self.view_name_entry = self.active_view.to_string();
        self.name_edit = NameEdit::Arming;
    }

    fn rename_view(&mut self) {
        let Some(new) = EntryName::forge(&self.view_name_entry) else {
            "rename needs a nonempty view name".clone_into(&mut self.status);
            return;
        };
        let old = self.active_view.clone();
        if self.rename_view_to(&old, new) {
            self.view_name_entry.clear();
            self.name_edit = NameEdit::Idle;
        }
    }

    fn rename_view_to(&mut self, old: &EntryName, new: EntryName) -> bool {
        if old == &new {
            return true;
        }
        if self.views.taken(&new) {
            self.status = format!("view `{new}` already exists");
            return false;
        }
        if old == &self.active_view {
            self.sync_active_view();
        }
        if !self.views.rename(old, new.clone()) {
            return false;
        }
        self.map_undo.rename_view(old, &new);
        if old == &self.active_view {
            self.active_view = new.clone();
            self.session_state.active_view = Some(new.clone());
        }
        self.status = format!("renamed view `{old}` → `{new}`");
        self.mark_dirty();
        self.mark_views_dirty();
        true
    }

    fn strike_overlay(&mut self, product: Product) {
        self.session_state.overlay = self.session_state.overlay.strike(product);
        self.mark_dirty();
        if let Some(product) = self.session_state.overlay.active() {
            if let Some(latest) = self.latest_runs.get(&product.system()).copied() {
                let selection = self.session_state.cycle.rectify(latest);
                let run =
                    ForecastRun::forge(product.system(), selection.bind(product.system(), latest));
                self.land_run(run);
            } else {
                self.run = None;
                self.demand_active();
                self.request_discovery();
            }
        } else {
            self.demand_id.advance();
            self.loading = None;
            self.displayed_field = None;
            self.prefetch.clear();
            "map only".clone_into(&mut self.status);
        }
    }

    fn choose_lead(&mut self, lead: LeadHour) {
        let Some((run, product, frontier)) = self.run.and_then(|run| {
            Some((
                run,
                self.session_state.overlay.active()?,
                self.frontier_for(run)?,
            ))
        }) else {
            return;
        };
        let floor = self
            .session_state
            .overlay
            .active()
            .filter(|product| product.has_baseline())
            .map_or(Some(LeadHour::ZERO), |_| self.session_state.base.next());
        let Some(floor) = floor.filter(|floor| *floor <= frontier) else {
            return;
        };
        let bounded = lead.clamp(floor, frontier);
        let Some(lead) = product
            .lead_axis(run)
            .ok()
            .and_then(|axis| axis.snap(bounded, frontier))
            .filter(|lead| *lead >= floor)
        else {
            return;
        };
        if self.session_state.lead != lead {
            self.session_state.lead = lead;
            self.mark_dirty();
            self.demand_active();
        }
    }

    fn choose_base(&mut self, base: LeadHour) {
        let cumulative = self
            .session_state
            .overlay
            .active()
            .is_some_and(Product::has_baseline);
        if !cumulative || self.session_state.lead == LeadHour::ZERO {
            return;
        }
        let base = base.min(self.session_state.lead.saturating_previous());
        if self.session_state.base != base {
            self.session_state.base = base;
            self.mark_dirty();
            self.demand_active();
        }
    }

    fn follow_cycle(&mut self, selection: RunSelection) {
        if self.session_state.cycle != selection {
            self.session_state.cycle = selection;
            self.mark_dirty();
        }
        if let Some(system) = self.session_state.overlay.active().map(Product::system)
            && let Some(latest) = self.latest_runs.get(&system).copied()
        {
            let run = ForecastRun::forge(system, selection.rectify(latest).bind(system, latest));
            if self.run != Some(run) {
                self.land_run(run);
            }
        }
        self.request_discovery();
    }

    fn land_run(&mut self, run: ForecastRun) {
        if self.run == Some(run) {
            self.request_survey(run);
        } else {
            self.rebase_forecast(run);
            self.mark_dirty();
            if self.run_extents.contains_key(&run) {
                self.clamp_clock();
                self.demand_active();
            } else {
                self.demand_id.advance();
                self.prefetch.clear();
                self.request_survey(run);
            }
        }
    }

    fn reconcile_forecast(&mut self) {
        self.clamp_clock();
        let active = self.active_key();
        if self.session_state.overlay.active().is_some()
            && (active.is_none()
                || self
                    .displayed_field
                    .as_ref()
                    .is_none_or(|(key, _field)| Some(*key) != active))
        {
            self.demand_active();
        }
    }

    fn clamp_clock(&mut self) {
        let Some(run) = self.run else {
            return;
        };
        let Some(ceiling) = self.frontier_for(run).or_else(|| {
            self.session_state
                .overlay
                .active()
                .and_then(|product| product.horizon(run).ok())
        }) else {
            return;
        };
        let product = self.session_state.overlay.active();
        let (mut lead, base) = lawful_clock(
            product,
            self.session_state.lead,
            self.session_state.base,
            ceiling,
        );
        if let Some(axis) = product.and_then(|product| product.lead_axis(run).ok())
            && let Some(snapped) = axis.snap(lead, ceiling)
        {
            lead = snapped;
        }
        if (lead, base) != (self.session_state.lead, self.session_state.base) {
            self.session_state.lead = lead;
            self.session_state.base = base;
            self.mark_dirty();
        }
    }

    fn rebase_forecast(&mut self, run: ForecastRun) {
        let frontier = self.frontier_for(run).unwrap_or_else(|| {
            self.session_state
                .overlay
                .active()
                .and_then(|product| product.horizon(run).ok())
                .unwrap_or(LeadHour::ZERO)
        });
        let (lead, base) = self.run.map_or(
            (self.session_state.lead, self.session_state.base),
            |source| {
                (
                    run.rebase_lead(source, self.session_state.lead, frontier),
                    run.rebase_lead(source, self.session_state.base, frontier),
                )
            },
        );
        let product = self.session_state.overlay.active();
        let (mut lead, base) = lawful_clock(product, lead, base, frontier);
        if let Some(axis) = product.and_then(|product| product.lead_axis(run).ok())
            && let Some(snapped) = axis.snap(lead, frontier)
        {
            lead = snapped;
        }
        self.run = Some(run);
        self.session_state.lead = lead;
        self.session_state.base = base;
    }

    fn active_key(&self) -> Option<FrameKey> {
        let run = self.run?;
        let product = self.session_state.overlay.active()?;
        let published = self.frontier_for(run)?;
        product
            .lead_axis(run)
            .ok()?
            .ready(self.session_state.lead, published)
            .then(|| {
                FrameKey::forge(
                    run,
                    product,
                    self.session_state.base,
                    self.session_state.lead,
                )
            })
            .flatten()
    }

    fn frontier_for(&self, run: ForecastRun) -> Option<LeadHour> {
        let product = self.session_state.overlay.active()?;
        (product.system() == run.system).then_some(())?;
        let horizon = product.horizon(run).ok()?;
        self.run_extents
            .get(&run)
            .copied()
            .map(|published| published.min(horizon))
    }

    fn scale_for(&self, key: FrameKey) -> &Scale {
        self.scales.get(
            key.product,
            self.range_scale.settled(),
            TemperatureSeason::at(key),
        )
    }

    fn demand_cover(&mut self, cover: &basemap::Cover) {
        if let Some(fallback) = cover.strata.first() {
            for &key in &fallback.keys {
                self.demand_tile(key);
            }
        }
        for stratum in &cover.strata {
            if stratum.intent.demands() {
                for &key in &stratum.keys {
                    self.demand_tile(key);
                }
            }
        }
        for stratum in cover.strata.iter().rev() {
            if stratum.intent == basemap::Intent::Retained {
                for &key in &stratum.keys {
                    self.demand_tile(key);
                }
            }
        }
    }

    fn demand_tile(&mut self, key: TileKey) {
        let rejected = match self.tile_rejections.get(&key).copied() {
            Some(rejection) if rejection.blocks(Instant::now()) => true,
            Some(_) => {
                let _expired = self.tile_rejections.remove(&key);
                false
            }
            None => false,
        };
        if !self.tiles.contains(key)
            && !self.tile_inflight.contains(&key)
            && !rejected
            && self.basemap.request(key)
        {
            let _fresh = self.tile_inflight.insert(key);
        }
    }

    fn paint_labels(&self, painter: &egui::Painter, map_rect: egui::Rect) {
        let mut candidates = self
            .presented_basemap
            .iter()
            .flat_map(|tile| tile.labels.iter())
            .collect::<Vec<_>>();
        candidates.sort_unstable_by_key(|label| label.rank);
        let mut occupied = Vec::<egui::Rect>::new();
        for label in candidates {
            let maturity = basemap::apparition(self.viewport.zoom as f32, label.onset_zoom);
            if maturity <= 0.01 {
                continue;
            }
            let anchor = map::screen_at(self.viewport, map_rect, label.world);
            let size = label.size * (0.88 + 0.12 * maturity);
            let width = label.text.chars().count() as f32 * size * 0.58;
            let footprint =
                egui::Rect::from_center_size(anchor, egui::vec2(width.max(size), size * 1.25))
                    .expand(2.0);
            if !map_rect.contains_rect(footprint)
                || occupied.iter().any(|prior| prior.intersects(footprint))
            {
                continue;
            }
            occupied.push(footprint);
            if occupied.len() >= 180 {
                break;
            }
            let font = chrome::spatial_font(painter.ctx(), size, egui::FontFamily::Proportional);
            let halo = Color32::from_rgba_unmultiplied(
                basemap::PAPER_SRGB[0],
                basemap::PAPER_SRGB[1],
                basemap::PAPER_SRGB[2],
                (178.0 * maturity) as u8,
            );
            for offset in [
                egui::vec2(-1.0, 0.0),
                egui::vec2(1.0, 0.0),
                egui::vec2(0.0, -1.0),
                egui::vec2(0.0, 1.0),
                egui::vec2(-0.7, -0.7),
                egui::vec2(0.7, -0.7),
                egui::vec2(-0.7, 0.7),
                egui::vec2(0.7, 0.7),
            ] {
                let _halo = painter.text(
                    anchor + offset,
                    egui::Align2::CENTER_CENTER,
                    label.text.as_ref(),
                    font.clone(),
                    halo,
                );
            }
            let _label = painter.text(
                anchor,
                egui::Align2::CENTER_CENTER,
                label.text.as_ref(),
                font,
                Color32::from_black_alpha((215.0 * maturity) as u8),
            );
        }
    }

    fn active_field(&self) -> Option<(FrameKey, Arc<FieldGrid>)> {
        let key = self.active_key()?;
        self.fields.get(key).map(|field| (key, field.clone()))
    }

    fn demand_active(&mut self) {
        let Some(key) = self.active_key() else {
            self.demand_id.advance();
            self.loading = None;
            self.prefetch.clear();
            self.displayed_field = None;
            return;
        };
        self.demand_id.advance();
        self.prefetch.clear();
        self.seed_prefetch(key);
        if self.fields.get(key).is_some() {
            self.loading = None;
            self.displayed_field = self.fields.get(key).map(|field| (key, field.clone()));
            "forecast ready".clone_into(&mut self.status);
            self.kick_prefetch();
        } else {
            let demand = LoadDemand {
                intent: LoadIntent::Foreground(self.demand_id),
                key,
            };
            self.loading = Some(demand);
            "loading forecast…".clone_into(&mut self.status);
            if let Err(err) = self.worker.send(Command::Load(demand)) {
                self.loading = None;
                self.status = err.to_string();
            }
        }
    }

    fn seed_prefetch(&mut self, key: FrameKey) {
        let Some((axis, ceiling)) = key
            .product
            .lead_axis(key.run)
            .ok()
            .zip(self.frontier_for(key.run))
            .and_then(|(axis, frontier)| Some((axis, axis.ready_ceiling(frontier)?)))
        else {
            return;
        };
        let current = axis.index_at_or_before(key.valid);
        for distance in 1_u16..=2 {
            if let Some(next) = current
                .checked_add(distance)
                .filter(|next| *next <= ceiling)
                && let Some(lead) = axis.at(next)
                && let Some(frame) = key.with_valid(lead)
            {
                self.prefetch.push_back(frame);
            }
            if let Some(previous) = current.checked_sub(distance)
                && let Some(lead) = axis.at(previous)
                && let Some(frame) = key.with_valid(lead)
            {
                self.prefetch.push_back(frame);
            }
        }
    }

    fn kick_prefetch(&mut self) {
        if self.loading.is_some() {
            return;
        }
        while let Some(key) = self.prefetch.pop_front() {
            if self.fields.get(key).is_some() {
                continue;
            }
            let demand = LoadDemand {
                intent: LoadIntent::Prefetch,
                key,
            };
            if self.worker.send(Command::Load(demand)).is_ok() {
                self.loading = Some(demand);
            }
            break;
        }
    }

    fn mark_dirty(&mut self) {
        self.dirty.session_state = true;
        self.scribe.mark();
    }

    fn mark_views_dirty(&mut self) {
        self.dirty.views = true;
        self.scribe.mark();
    }

    fn durable_state(&self) -> DurableState {
        DurableState {
            session_state: self.dirty.session_state.then(|| self.session_state.clone()),
            views: self.dirty.views.then(|| self.views.clone()),
        }
    }

    fn durable_state_all(&self) -> DurableState {
        DurableState {
            session_state: Some(self.session_state.clone()),
            views: Some(self.views.clone()),
        }
    }

    fn absorb_persistence(&mut self) {
        if let Some(ScribeOutcome::Fault { message, .. }) = self.scribe.take_outcome() {
            self.dirty = DirtyState {
                session_state: true,
                views: true,
            };
            self.status = format!("state save failed: {message}");
        }
    }
}

impl Drop for WeatherApp {
    fn drop(&mut self) {
        self.retire();
    }
}

fn consume_view_slot(ctx: &egui::Context) -> Option<(ViewSlot, bool)> {
    ctx.input_mut(|input| {
        let (index, command) =
            input.events.iter().enumerate().find_map(|(index, event)| {
                view_slot_command(event).map(|command| (index, command))
            })?;
        let _event = input.events.remove(index);
        Some(command)
    })
}

fn lawful_clock(
    product: Option<Product>,
    mut lead: LeadHour,
    mut base: LeadHour,
    ceiling: LeadHour,
) -> (LeadHour, LeadHour) {
    lead = lead.min(ceiling);
    if product.is_some_and(Product::has_baseline) {
        if ceiling == LeadHour::ZERO {
            return (LeadHour::ZERO, LeadHour::ZERO);
        }
        if lead == LeadHour::ZERO {
            lead = LeadHour::ONE;
        }
        base = base.min(lead.saturating_previous());
    }
    (lead, base)
}

#[cfg(feature = "egui-test")]
fn witnessed_frontier(run: Option<ForecastRun>) -> Option<(ForecastRun, LeadHour)> {
    let run = run?;
    let raw = std::env::var("HRRR_ACCEPTANCE_PUBLISHED").ok()?;
    let published = raw
        .parse::<u8>()
        .ok()
        .and_then(|hour| LeadHour::forge(hour).ok())?;
    Some((run, published))
}

fn view_slot_command(event: &egui::Event) -> Option<(ViewSlot, bool)> {
    let egui::Event::Key {
        key,
        physical_key,
        pressed: true,
        repeat: false,
        modifiers,
    } = event
    else {
        return None;
    };
    if modifiers.alt || modifiers.ctrl || modifiers.command || modifiers.mac_cmd {
        return None;
    }
    let digit = match physical_key.unwrap_or(*key) {
        egui::Key::Num0 => 0,
        egui::Key::Num1 => 1,
        egui::Key::Num2 => 2,
        egui::Key::Num3 => 3,
        egui::Key::Num4 => 4,
        egui::Key::Num5 => 5,
        egui::Key::Num6 => 6,
        egui::Key::Num7 => 7,
        egui::Key::Num8 => 8,
        egui::Key::Num9 => 9,
        _ => return None,
    };
    ViewSlot::forge(digit).map(|slot| (slot, modifiers.shift))
}

fn apportion_product_widths(ui: &egui::Ui, row: &[Product]) -> [f32; Product::ALL.len()] {
    debug_assert!(!row.is_empty(), "a product row holds at least one product");
    let gap = ui.spacing().item_spacing.x * row.len().saturating_sub(1) as f32;
    let available = (ui.available_width() - gap).max(0.0);
    let style = ui.style().button_style(
        &egui::widget_style::Classes::default(),
        egui::widget_style::WidgetState::Inactive,
    );
    let rim = style.frame.total_margin().sum().x;
    let mut minima = [0.0; Product::ALL.len()];
    for (index, product) in row.iter().enumerate() {
        minima[index] = ui
            .painter()
            .layout_no_wrap(
                product.label().to_owned(),
                style.text_style.font_id.clone(),
                style.text_style.color,
            )
            .size()
            .x
            + rim;
    }

    let minimum = minima[..row.len()].iter().sum::<f32>();
    let mut widths = minima;
    if minimum > available {
        let contraction = available / minimum;
        for width in &mut widths[..row.len()] {
            *width *= contraction;
        }
        return widths;
    }

    let mut fluid = [true; Product::ALL.len()];
    let mut fluid_count = row.len();
    let mut remainder = available;
    loop {
        let share = remainder / fluid_count as f32;
        let mut claimed = false;
        for index in 0..row.len() {
            if fluid[index] && minima[index] > share {
                fluid[index] = false;
                fluid_count -= 1;
                remainder -= minima[index];
                claimed = true;
            }
        }
        if !claimed {
            for index in 0..row.len() {
                if fluid[index] {
                    widths[index] = share;
                }
            }
            return widths;
        }
    }
}

/// A Panel's title in the Dock and Drawer.
const fn title(panel: Panel) -> &'static str {
    match panel {
        Panel::Application => "application",
        Panel::Field => "field",
        Panel::Forecast => "forecast",
        Panel::ActiveView => "active view",
        Panel::Views => "views",
        Panel::Status => "status",
    }
}

struct FrameBank {
    capacity: usize,
    fields: HashMap<FrameKey, Arc<FieldGrid>>,
    order: VecDeque<FrameKey>,
}

impl FrameBank {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            fields: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get(&self, key: FrameKey) -> Option<&Arc<FieldGrid>> {
        self.fields.get(&key.field_identity())
    }

    fn insert(&mut self, key: FrameKey, field: Arc<FieldGrid>) {
        let key = key.field_identity();
        if self.fields.insert(key, field).is_none() {
            self.order.push_back(key);
        }
        while self.fields.len() > self.capacity {
            if let Some(victim) = self.order.pop_front() {
                let _evicted = self.fields.remove(&victim);
            }
        }
    }
}

struct VectorBank {
    ceiling: usize,
    bytes: usize,
    epoch: u64,
    tiles: HashMap<TileKey, VectorEntry>,
}

struct VectorEntry {
    tile: Arc<VectorTile>,
    bytes: usize,
    touched: u64,
}

impl VectorBank {
    fn new(ceiling: usize) -> Self {
        Self {
            ceiling,
            bytes: 0,
            epoch: 0,
            tiles: HashMap::new(),
        }
    }

    fn get(&mut self, key: TileKey) -> Option<&Arc<VectorTile>> {
        self.epoch = self.epoch.saturating_add(1);
        let entry = self.tiles.get_mut(&key)?;
        entry.touched = self.epoch;
        Some(&entry.tile)
    }

    fn contains(&self, key: TileKey) -> bool {
        self.tiles.contains_key(&key)
    }

    fn insert(&mut self, tile: Arc<VectorTile>) {
        let key = tile.key;
        let bytes = tile.resident_bytes();
        self.epoch = self.epoch.saturating_add(1);
        let fresh = VectorEntry {
            tile,
            bytes,
            touched: self.epoch,
        };
        if let Some(prior) = self.tiles.insert(key, fresh) {
            self.bytes = self.bytes.saturating_sub(prior.bytes);
        }
        self.bytes = self.bytes.saturating_add(bytes);
        while self.bytes > self.ceiling && self.tiles.len() > 1 {
            let victim = self
                .tiles
                .iter()
                .min_by_key(|(_key, entry)| entry.touched)
                .map(|(key, _entry)| *key);
            let Some(victim) = victim else { break };
            let Some(victim) = self.tiles.remove(&victim) else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(victim.bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context as _;

    fn point(x: f64) -> Result<MercatorPoint> {
        MercatorPoint::forge([x, 0.5]).context("test map point")
    }

    fn view(name: &str) -> Result<EntryName> {
        EntryName::forge(name).context("test view name")
    }

    #[test]
    fn scale_latch_requires_quiet_after_navigation() {
        // A pan can cross both range thresholds in one frame. Value hysteresis
        // alone therefore cannot keep the field and legend stable under drag.
        let begun = Instant::now();
        let mut latch = ScaleLatch::<RangeRegime>::default();

        assert_eq!(latch.observe(RangeRegime::Broad, begun), Some(SCALE_SETTLE));
        latch.arrest();
        let released = begun + SCALE_SETTLE;
        assert_eq!(
            latch.observe(RangeRegime::Broad, released),
            Some(SCALE_SETTLE)
        );
        assert_eq!(
            latch.observe(
                RangeRegime::Broad,
                released + SCALE_SETTLE.saturating_sub(Duration::from_millis(1)),
            ),
            Some(Duration::from_millis(1))
        );
        assert_eq!(latch.settled(), RangeRegime::Fine);
        assert_eq!(
            latch.observe(RangeRegime::Broad, released + SCALE_SETTLE),
            None
        );
        assert_eq!(latch.settled(), RangeRegime::Broad);
    }

    #[test]
    fn map_undo_interleaves_global_probe_and_view_owned_pins() -> Result<()> {
        let alpha = view("alpha")?;
        let beta = view("beta")?;
        let alpha_pin = point(0.2)?;
        let beta_pin = point(0.7)?;
        let probe = point(0.4)?;
        let mut undo = MapUndo::default();
        undo.remember(MapReversal::Pins {
            view: alpha.clone(),
            before: Vec::new(),
        });
        undo.remember(MapReversal::Pins {
            view: beta.clone(),
            before: Vec::new(),
        });
        undo.remember(MapReversal::Probe { before: None });

        assert_eq!(
            undo.recoil(&alpha, &[alpha_pin], Some(probe)),
            Some(MapRecoil::Probe(None))
        );
        assert_eq!(
            undo.recoil(&alpha, &[alpha_pin], None),
            Some(MapRecoil::Pins(Vec::new()))
        );
        assert_eq!(undo.recoil(&alpha, &[], None), None);
        assert_eq!(
            undo.recoil(&beta, &[beta_pin], None),
            Some(MapRecoil::Pins(Vec::new()))
        );
        Ok(())
    }

    #[test]
    fn map_undo_tracks_view_renames_and_forgets_deleted_views() -> Result<()> {
        let old = view("old")?;
        let new = view("new")?;
        let pin = point(0.2)?;
        let mut undo = MapUndo::default();
        undo.remember(MapReversal::Pins {
            view: old.clone(),
            before: Vec::new(),
        });
        undo.rename_view(&old, &new);
        assert_eq!(
            undo.recoil(&new, &[pin], None),
            Some(MapRecoil::Pins(Vec::new()))
        );

        undo.remember(MapReversal::Pins {
            view: new.clone(),
            before: Vec::new(),
        });
        undo.remember(MapReversal::Probe { before: None });
        undo.forget_view(&new);
        assert_eq!(
            undo.recoil(&new, &[], Some(pin)),
            Some(MapRecoil::Probe(None))
        );
        assert_eq!(undo.recoil(&new, &[pin], None), None);
        Ok(())
    }

    #[test]
    fn physical_digits_survive_shifted_logical_keys() {
        let shifted = egui::Event::Key {
            key: egui::Key::Exclamationmark,
            physical_key: Some(egui::Key::Num1),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::SHIFT,
        };
        let command = view_slot_command(&shifted).map(|(slot, assign)| (slot.digit(), assign));
        assert_eq!(command, Some((1, true)));

        let plain = egui::Event::Key {
            key: egui::Key::Num7,
            physical_key: Some(egui::Key::Num7),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let command = view_slot_command(&plain).map(|(slot, assign)| (slot.digit(), assign));
        assert_eq!(command, Some((7, false)));
    }

    #[test]
    fn absent_tiles_are_final_while_faults_cool_down() {
        let now = Instant::now();
        assert!(TileRejection::Absent.blocks(now));
        assert!(TileRejection::Absent.resolves());
        assert!(TileRejection::RetryAt(now + TILE_RETRY_DELAY).blocks(now));
        assert!(!TileRejection::RetryAt(now).resolves());
        assert!(!TileRejection::RetryAt(now).blocks(now));
    }

    #[test]
    fn cumulative_clock_preserves_valid_time_and_lowers_an_illegal_base() -> Result<()> {
        let hour = |value| LeadHour::forge(value);
        assert_eq!(
            lawful_clock(Some(Product::QpfRun), hour(5)?, hour(8)?, hour(18)?),
            (hour(5)?, hour(4)?)
        );
        assert_eq!(
            lawful_clock(Some(Product::QpfRun), hour(0)?, hour(8)?, hour(18)?),
            (hour(1)?, hour(0)?)
        );
        assert_eq!(
            lawful_clock(Some(Product::QpfRun), hour(8)?, hour(3)?, hour(0)?),
            (hour(0)?, hour(0)?)
        );
        assert_eq!(
            lawful_clock(Some(Product::Smoke), hour(8)?, hour(6)?, hour(4)?),
            (hour(4)?, hour(6)?)
        );
        Ok(())
    }
}
