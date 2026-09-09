use std::{env, ffi::OsString, path::PathBuf, time::Duration};

use egui_tester::{
    AndroidChoreography, AndroidDevice, AndroidPerfetto, AndroidPinch, Choreography, Error, Result,
    Silent, StoryEvent, StoryFact, StoryObserver, StorySurface,
};
use hrrr_choreography::{
    Act, Direction, Field, HrrrProjection, Modal, Panel, RunTether, Score, ViewAct, Zoom, perform,
};

mod report;

const PACKAGE: &str = "moe.swarm.hrrr";

fn main() -> Result<()> {
    let cli = Cli::parse()?;
    let device = AndroidDevice::new(cli.serial).adb(cli.adb);
    device.install_driver(cli.driver)?;
    device.force_stop(PACKAGE)?;
    if cli.fresh {
        device.clear_app_data(PACKAGE)?;
    }
    device.start_native_activity(PACKAGE)?;
    std::thread::sleep(Duration::from_secs(if cli.fresh { 8 } else { 3 }));
    let screen = Screen::refine(device.screen_size()?)?;

    if cli.profile.is_some() && cli.captures.is_some() {
        return Err(verdict(
            "--profile and --captures are separate projections; screenshots would contaminate the performance trace",
        ));
    }

    if let Some(profile) = cli.profile {
        let config = std::fs::read_to_string(&profile.config).map_err(|source| Error::Io {
            operation: "read Android Perfetto configuration",
            path: profile.config,
            source,
        })?;
        let observer = AndroidPerfetto::begin(
            &device,
            &config,
            "/data/misc/perfetto-traces/hrrr-comprehensive.pftrace",
            &profile.trace,
            &profile.events,
        )?;
        let _observer = run(&device, screen, cli.score, observer)?;
        report::write(
            &cli.trace_processor,
            &profile.trace,
            &profile.events,
            &profile.report,
            cli.score,
        )
    } else if let Some(captures) = cli.captures {
        let observer = CaptureObserver::forge(captures)?;
        let _observer = run(&device, screen, cli.score, observer)?;
        Ok(())
    } else {
        let _observer = run(&device, screen, cli.score, Silent)?;
        Ok(())
    }
}

struct CaptureObserver {
    directory: PathBuf,
    sequence: usize,
}

impl CaptureObserver {
    fn forge(directory: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&directory).map_err(|source| Error::Io {
            operation: "create Android visual-trace directory",
            path: directory.clone(),
            source,
        })?;
        Ok(Self {
            directory,
            sequence: 0,
        })
    }

    fn capture(&mut self, action: &str, surface: StorySurface<'_>) -> Result<()> {
        std::thread::sleep(Duration::from_millis(180));
        self.sequence += 1;
        let slug = action
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() {
                    character.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect::<String>();
        surface.capture()?.save_png(
            self.directory
                .join(format!("{:03}-{slug}.png", self.sequence)),
        )
    }
}

impl StoryObserver for CaptureObserver {
    fn observe(&mut self, event: StoryEvent<'_>, surface: StorySurface<'_>) -> Result<()> {
        if let StoryEvent::Fact(StoryFact::ActionDispatched { action, .. }) = event {
            self.capture(action, surface)?;
        }
        Ok(())
    }

    fn finish(&mut self, surface: StorySurface<'_>) -> Result<()> {
        self.capture("final", surface)
    }
}

fn run<O: StoryObserver>(
    device: &AndroidDevice,
    screen: Screen,
    score: Score,
    observer: O,
) -> Result<O> {
    let story = AndroidChoreography::bind(device)?.with_observer(observer);
    let mut android = AndroidHrrr::new(device, story, screen);
    perform(&mut android, score)?;
    android.finish()
}

struct AndroidHrrr<'device, O> {
    device: &'device AndroidDevice,
    story: AndroidChoreography<'device, O>,
    screen: Screen,
    field_sampled: bool,
    field_scrolled: bool,
    forecast_scrolled: bool,
    probe: [i32; 2],
    pin: [i32; 2],
    pin_reversal: PinReversal,
    folders: u8,
}

impl<'device, O: StoryObserver> AndroidHrrr<'device, O> {
    fn new(
        device: &'device AndroidDevice,
        story: AndroidChoreography<'device, O>,
        screen: Screen,
    ) -> Self {
        Self {
            device,
            story,
            screen,
            field_sampled: false,
            field_scrolled: false,
            forecast_scrolled: false,
            probe: screen.at(0.50, 0.35),
            pin: screen.at(0.38, 0.34),
            pin_reversal: PinReversal::None,
            folders: 0,
        }
    }

    fn finish(self) -> Result<O> {
        self.device.require_resumed(PACKAGE)?;
        self.story.finish()
    }

    fn settle(&self, duration: Duration) {
        self.story.settle(duration);
    }

    fn swipe_panel(
        &mut self,
        action: &str,
        destination: Panel,
        direction: SwipeDirection,
    ) -> Result<()> {
        let (origin, end) = match direction {
            SwipeDirection::Left => (self.screen.at(0.78, 0.75), self.screen.at(0.22, 0.75)),
            SwipeDirection::Right => (self.screen.at(0.22, 0.75), self.screen.at(0.78, 0.75)),
        };
        let _receipt = self.story.swipe(
            &format!("{action} {} panel", destination.name()),
            origin,
            end,
            Duration::from_millis(220),
        )?;
        self.settle(Duration::from_millis(140));
        Ok(())
    }
}

impl<O: StoryObserver> Choreography for AndroidHrrr<'_, O> {
    fn chapter(&mut self, title: &str) -> Result<()> {
        self.story.chapter(title)
    }

    fn hold(&mut self, duration: Duration) -> Result<()> {
        self.story.hold(duration)?;
        self.story.settle(duration);
        Ok(())
    }

    fn tempo(&mut self, tempo: egui_tester::StoryTempo) -> Result<()> {
        self.story.tempo(tempo)
    }
}

impl<O: StoryObserver> AndroidHrrr<'_, O> {
    fn select_panel(&mut self, panel: Panel) -> Result<()> {
        for _ in 0..6 {
            let _receipt = self
                .story
                .tap("seek first Inspector panel", self.screen.at(0.055, 0.574))?;
            self.settle(Duration::from_millis(55));
        }
        for _ in 0..panel.index() {
            let _receipt = self.story.tap(
                &format!("seek {} panel", panel.name()),
                self.screen.at(0.945, 0.574),
            )?;
            self.settle(Duration::from_millis(70));
        }
        self.field_scrolled = false;
        self.forecast_scrolled = false;
        self.settle(Duration::from_millis(180));
        Ok(())
    }

    fn lower_inspector(&mut self) -> Result<()> {
        let _receipt = self
            .story
            .tap("lower Inspector", self.screen.at(0.50, 0.574))?;
        self.settle(Duration::from_millis(350));
        Ok(())
    }

    fn raise_inspector(&mut self) -> Result<()> {
        let _receipt = self
            .story
            .tap("raise Inspector", self.screen.at(0.50, 0.929))?;
        self.settle(Duration::from_millis(350));
        Ok(())
    }

    fn next_panel(&mut self, panel: Panel) -> Result<()> {
        self.swipe_panel("swipe to", panel, SwipeDirection::Left)
    }

    fn previous_panel(&mut self, panel: Panel) -> Result<()> {
        self.swipe_panel("swipe back to", panel, SwipeDirection::Right)
    }

    fn open_settings(&mut self) -> Result<()> {
        let _receipt = self
            .story
            .tap("open Settings", self.screen.at(0.95, 0.66))?;
        self.settle(Duration::from_millis(500));
        Ok(())
    }

    fn open_command_guide(&mut self) -> Result<()> {
        let _receipt = self
            .story
            .tap("open command guide", self.screen.at(0.88, 0.66))?;
        self.settle(Duration::from_millis(500));
        Ok(())
    }

    fn close_modal(&mut self, surface: Modal) -> Result<()> {
        let _receipt = self.story.tap(
            surface_close_action(surface),
            self.screen
                .at(surface_close_x(surface), surface_close_y(surface)),
        )?;
        self.settle(Duration::from_millis(300));
        self.device.require_resumed(PACKAGE)
    }

    fn pan_map(&mut self) -> Result<()> {
        let _receipt = self.story.swipe(
            "pan map north-west",
            self.screen.at(0.58, 0.38),
            self.screen.at(0.43, 0.31),
            Duration::from_millis(320),
        )?;
        self.settle(Duration::from_millis(250));
        Ok(())
    }

    fn zoom_map(&mut self, direction: Zoom) -> Result<()> {
        let center_y = self.screen.at(0.50, 0.36)[1];
        let reach = self.screen.width / 12;
        let spread = match direction {
            Zoom::Out => self.screen.width / 14,
            Zoom::In => -self.screen.width / 28,
        };
        let _receipt = self.story.pinch(
            zoom_action(direction),
            AndroidPinch {
                first: [self.screen.width / 2 - reach, center_y],
                second: [self.screen.width / 2 + reach, center_y],
                spread: [spread, 0],
                steps: 12,
                duration: Duration::from_millis(240),
            },
        )?;
        self.settle(Duration::from_millis(350));
        Ok(())
    }

    fn select_field(&mut self, field: Field) -> Result<()> {
        if field == Field::AirQuality && !self.field_scrolled {
            let _receipt = self.story.swipe(
                "reveal air-quality field",
                self.screen.at(0.50, 0.91),
                self.screen.at(0.50, 0.70),
                Duration::from_millis(320),
            )?;
            self.field_scrolled = true;
            self.settle(Duration::from_millis(180));
        } else if field != Field::AirQuality && self.field_scrolled {
            let _receipt = self.story.swipe(
                "restore field panel origin",
                self.screen.at(0.50, 0.70),
                self.screen.at(0.50, 0.93),
                Duration::from_millis(320),
            )?;
            self.field_scrolled = false;
            self.settle(Duration::from_millis(180));
        }
        let _receipt = self.story.tap(
            field_action(field),
            self.screen.at(field_x(field), field_y(field)),
        )?;
        self.field_sampled = true;
        self.settle(Duration::from_millis(650));
        Ok(())
    }

    fn place_probe(&mut self) -> Result<()> {
        self.probe = self.screen.at(0.50, 0.35);
        let _receipt = self.story.tap("place transient probe", self.probe)?;
        self.settle(Duration::from_millis(300));
        Ok(())
    }

    fn move_probe(&mut self) -> Result<()> {
        self.probe = self.screen.at(0.61, 0.30);
        let _receipt = self.story.tap("move transient probe", self.probe)?;
        self.settle(Duration::from_millis(300));
        Ok(())
    }

    fn clear_probe(&mut self) -> Result<()> {
        let _receipt = self.story.tap("clear transient probe", self.probe)?;
        self.settle(Duration::from_millis(300));
        Ok(())
    }

    fn place_pin(&mut self) -> Result<()> {
        self.pin = self.screen.at(0.38, 0.34);
        let _receipt =
            self.story
                .long_press("place persistent pin", self.pin, Duration::from_millis(700))?;
        self.pin_reversal = PinReversal::None;
        self.settle(Duration::from_millis(300));
        Ok(())
    }

    fn drag_pin(&mut self) -> Result<()> {
        let destination = self.screen.at(0.58, 0.39);
        let grip_offset = self.screen.height * 3 / 100;
        let _receipt = self.story.swipe(
            "drag persistent pin",
            [self.pin[0], self.pin[1] - grip_offset],
            [destination[0], destination[1] - grip_offset],
            Duration::from_millis(320),
        )?;
        self.pin_reversal = PinReversal::Motion(self.pin);
        self.pin = destination;
        self.settle(Duration::from_millis(300));
        Ok(())
    }

    fn remove_pin(&mut self) -> Result<()> {
        let plaque_flank = if self.field_sampled { 42 } else { 37 };
        let close = [
            (self.pin[0] + self.screen.width * plaque_flank / 100).min(self.screen.width - 35),
            self.pin[1] - self.screen.height * 2 / 100,
        ];
        let _receipt = self.story.tap("remove persistent pin", close)?;
        self.pin_reversal = PinReversal::Removal;
        self.settle(Duration::from_millis(300));
        Ok(())
    }

    fn undo_map_change(&mut self) -> Result<()> {
        let _receipt = self
            .story
            .tap("undo map change", self.screen.at(0.94, 0.935))?;
        if let PinReversal::Motion(origin) = self.pin_reversal {
            self.pin = origin;
        }
        self.pin_reversal = PinReversal::None;
        self.settle(Duration::from_millis(300));
        Ok(())
    }

    fn restore_forecast_origin(&mut self) -> Result<()> {
        if !self.forecast_scrolled {
            return Ok(());
        }
        let _receipt = self.story.swipe(
            "restore forecast panel origin",
            self.screen.at(0.50, 0.69),
            self.screen.at(0.50, 0.93),
            Duration::from_millis(320),
        )?;
        self.forecast_scrolled = false;
        self.settle(Duration::from_millis(180));
        Ok(())
    }

    fn reveal_run_controls(&mut self) -> Result<()> {
        if self.forecast_scrolled {
            return Ok(());
        }
        let _receipt = self.story.swipe(
            "reveal forecast run controls",
            self.screen.at(0.50, 0.92),
            self.screen.at(0.50, 0.69),
            Duration::from_millis(320),
        )?;
        self.forecast_scrolled = true;
        self.settle(Duration::from_millis(180));
        Ok(())
    }

    fn step_forecast_hour(&mut self, direction: Direction) -> Result<()> {
        self.restore_forecast_origin()?;
        let _receipt = self.story.tap(
            forecast_step_action(direction),
            self.screen.at(
                match direction {
                    Direction::Previous => 0.055,
                    Direction::Next => 0.58,
                },
                0.687,
            ),
        )?;
        self.settle(Duration::from_millis(500));
        Ok(())
    }

    fn drag_forecast_hour(&mut self) -> Result<()> {
        self.restore_forecast_origin()?;
        let _receipt = self.story.swipe(
            "drag forecast-hour Rail",
            self.screen.at(0.20, 0.735),
            self.screen.at(0.52, 0.735),
            Duration::from_millis(420),
        )?;
        self.settle(Duration::from_millis(500));
        Ok(())
    }

    fn drag_base_hour(&mut self) -> Result<()> {
        self.restore_forecast_origin()?;
        let _receipt = self.story.swipe(
            "drag cumulative base-hour Rail",
            self.screen.at(0.10, 0.825),
            self.screen.at(0.28, 0.825),
            Duration::from_millis(420),
        )?;
        self.settle(Duration::from_millis(500));
        Ok(())
    }

    fn follow_run(&mut self, tether: RunTether) -> Result<()> {
        self.reveal_run_controls()?;
        let _receipt = self.story.tap(
            follow_run_action(tether),
            self.screen.at(
                0.50,
                match tether {
                    RunTether::Latest => 0.82,
                    RunTether::LatestLong => 0.865,
                },
            ),
        )?;
        self.settle(Duration::from_millis(600));
        Ok(())
    }

    fn step_run(&mut self, direction: Direction) -> Result<()> {
        self.reveal_run_controls()?;
        let _receipt = self.story.tap(
            run_step_action(direction),
            self.screen.at(
                match direction {
                    Direction::Previous => 0.25,
                    Direction::Next => 0.75,
                },
                0.92,
            ),
        )?;
        self.settle(Duration::from_millis(600));
        Ok(())
    }

    fn manage_views(&mut self, action: ViewAct) -> Result<()> {
        match action {
            ViewAct::RenameActive => {
                let _rename = self
                    .story
                    .tap("edit active view name", self.screen.at(0.05, 0.657))?;
                let _text = self.story.text("append active view name", " trace")?;
                self.settle(Duration::from_millis(180));
                let _commit = self.story.key_code("commit active view name", "ENTER")?;
                self.settle(Duration::from_millis(180));
                let _dismiss = self
                    .story
                    .key_code("dismiss active-view keyboard", "BACK")?;
                self.settle(Duration::from_millis(250));
            }
            ViewAct::Create => {
                let _create = self.story.tap("create view", self.screen.at(0.05, 0.692))?;
            }
            ViewAct::CreateFolder => {
                let y = match self.folders {
                    0 => 0.759,
                    _ => 0.813,
                };
                let _create = self
                    .story
                    .tap("create view folder", self.screen.at(0.06, y))?;
                self.folders = self.folders.saturating_add(1);
            }
            ViewAct::RenameFolder => {
                let _rename = self
                    .story
                    .tap("edit view folder name", self.screen.at(0.183, 0.771))?;
                let _text = self.story.text("append view folder name", " trace")?;
                self.settle(Duration::from_millis(180));
                let _commit = self.story.key_code("commit view folder name", "ENTER")?;
                self.settle(Duration::from_millis(180));
                let _dismiss = self.story.key_code("dismiss folder keyboard", "BACK")?;
                self.settle(Duration::from_millis(250));
            }
            ViewAct::ToggleFolder => {
                let _toggle = self
                    .story
                    .tap("toggle view folder", self.screen.at(0.102, 0.771))?;
            }
            ViewAct::RenameEntry => {
                let _rename = self
                    .story
                    .tap("edit saved view name", self.screen.at(0.083, 0.657))?;
                let _text = self.story.text("append saved view name", " trace")?;
                self.settle(Duration::from_millis(180));
                let _commit = self.story.key_code("commit saved view name", "ENTER")?;
                self.settle(Duration::from_millis(180));
                let _dismiss = self.story.key_code("dismiss saved-view keyboard", "BACK")?;
                self.settle(Duration::from_millis(250));
            }
            ViewAct::Clone => {
                let _clone = self
                    .story
                    .tap("clone saved view", self.screen.at(0.208, 0.692))?;
            }
            ViewAct::Load => {
                let _load = self
                    .story
                    .tap("load saved view", self.screen.at(0.35, 0.657))?;
            }
            ViewAct::DragEntryToFolder => {
                let _drag = self.story.swipe(
                    "drag saved view into folder",
                    self.screen.at(0.035, 0.727),
                    self.screen.at(0.40, 0.805),
                    Duration::from_millis(500),
                )?;
            }
            ViewAct::DragEntryToRoot => {
                let _drag = self.story.swipe(
                    "drag saved view back to root",
                    self.screen.at(0.058, 0.794),
                    self.screen.at(0.50, 0.727),
                    Duration::from_millis(500),
                )?;
            }
            ViewAct::DragFolder => {
                let _drag = self.story.swipe(
                    "reorder view folder",
                    self.screen.at(0.035, 0.862),
                    self.screen.at(0.40, 0.805),
                    Duration::from_millis(500),
                )?;
            }
            ViewAct::DeleteEntry => {
                let _delete = self
                    .story
                    .tap("delete disposable saved view", self.screen.at(0.145, 0.727))?;
            }
            ViewAct::DeleteFolder => {
                let _delete = self.story.tap(
                    "delete disposable view folder",
                    self.screen.at(0.267, 0.771),
                )?;
            }
        }
        self.settle(Duration::from_millis(360));
        Ok(())
    }
}

impl<O: StoryObserver> HrrrProjection for AndroidHrrr<'_, O> {
    fn enact(&mut self, act: Act) -> Result<()> {
        match act {
            Act::SelectPanel(panel) => self.select_panel(panel),
            Act::LowerInspector => self.lower_inspector(),
            Act::RaiseInspector => self.raise_inspector(),
            Act::NextPanel(panel) => self.next_panel(panel),
            Act::PreviousPanel(panel) => self.previous_panel(panel),
            Act::OpenModal(Modal::Settings) => self.open_settings(),
            Act::OpenModal(Modal::CommandGuide) => self.open_command_guide(),
            Act::CloseModal(surface) => self.close_modal(surface),
            Act::PanMap => self.pan_map(),
            Act::ZoomMap(direction) => self.zoom_map(direction),
            Act::PlaceProbe => self.place_probe(),
            Act::MoveProbe => self.move_probe(),
            Act::ClearProbe => self.clear_probe(),
            Act::PlacePin => self.place_pin(),
            Act::DragPin => self.drag_pin(),
            Act::RemovePin => self.remove_pin(),
            Act::UndoMapChange => self.undo_map_change(),
            Act::SelectField(field) => self.select_field(field),
            Act::StepForecastHour(direction) => self.step_forecast_hour(direction),
            Act::DragForecastHour => self.drag_forecast_hour(),
            Act::DragBaseHour => self.drag_base_hour(),
            Act::FollowRun(tether) => self.follow_run(tether),
            Act::StepRun(direction) => self.step_run(direction),
            Act::ManageViews(action) => self.manage_views(action),
        }
    }
}

#[derive(Clone, Copy)]
enum SwipeDirection {
    Left,
    Right,
}

#[derive(Clone, Copy, Default)]
enum PinReversal {
    #[default]
    None,
    Motion([i32; 2]),
    Removal,
}

const fn field_action(field: Field) -> &'static str {
    match field {
        Field::QpfTotal => "select total QPF field",
        Field::QpfHourly => "select hourly QPF field",
        Field::SurfaceSmoke => "select surface smoke field",
        Field::Temperature => "select temperature field",
        Field::DewPoint => "select dew-point field",
        Field::CloudCover => "select cloud-cover field",
        Field::SeaLevelPressure => "select sea-level pressure field",
        Field::Wind => "select wind field",
        Field::AirQuality => "select air-quality field",
    }
}

const fn field_x(field: Field) -> f32 {
    match field {
        Field::QpfTotal | Field::Temperature | Field::CloudCover => 0.255,
        Field::QpfHourly | Field::DewPoint | Field::SeaLevelPressure => 0.745,
        Field::SurfaceSmoke | Field::Wind | Field::AirQuality => 0.50,
    }
}

const fn field_y(field: Field) -> f32 {
    match field {
        Field::QpfTotal | Field::QpfHourly => 0.668,
        Field::SurfaceSmoke => 0.725,
        Field::Temperature | Field::DewPoint => 0.782,
        Field::CloudCover | Field::SeaLevelPressure => 0.840,
        Field::Wind => 0.895,
        Field::AirQuality => 0.905,
    }
}

const fn forecast_step_action(direction: Direction) -> &'static str {
    match direction {
        Direction::Previous => "select previous forecast hour",
        Direction::Next => "select next forecast hour",
    }
}

const fn follow_run_action(tether: RunTether) -> &'static str {
    match tether {
        RunTether::Latest => "follow latest forecast run",
        RunTether::LatestLong => "follow latest long forecast run",
    }
}

const fn run_step_action(direction: Direction) -> &'static str {
    match direction {
        Direction::Previous => "select older forecast run",
        Direction::Next => "select newer forecast run",
    }
}

const fn surface_close_action(surface: Modal) -> &'static str {
    match surface {
        Modal::Settings => "close Settings",
        Modal::CommandGuide => "close command guide",
    }
}

const fn surface_close_x(surface: Modal) -> f32 {
    match surface {
        Modal::Settings => 0.938,
        Modal::CommandGuide => 0.910,
    }
}

const fn surface_close_y(surface: Modal) -> f32 {
    match surface {
        Modal::Settings => 0.430,
        Modal::CommandGuide => 0.172,
    }
}

const fn zoom_action(zoom: Zoom) -> &'static str {
    match zoom {
        Zoom::In => "pinch map inward",
        Zoom::Out => "pinch map outward",
    }
}

#[derive(Clone, Copy, Debug)]
struct Screen {
    width: i32,
    height: i32,
}

impl Screen {
    fn refine([width, height]: [u32; 2]) -> Result<Self> {
        Ok(Self {
            width: i32::try_from(width).map_err(|_| verdict("Android width exceeds i32"))?,
            height: i32::try_from(height).map_err(|_| verdict("Android height exceeds i32"))?,
        })
    }

    fn at(self, x: f32, y: f32) -> [i32; 2] {
        [
            (self.width as f32 * x).round() as i32,
            (self.height as f32 * y).round() as i32,
        ]
    }
}

struct Cli {
    serial: String,
    adb: PathBuf,
    driver: PathBuf,
    score: Score,
    fresh: bool,
    captures: Option<PathBuf>,
    profile: Option<Profile>,
    trace_processor: PathBuf,
}

struct Profile {
    config: PathBuf,
    trace: PathBuf,
    events: PathBuf,
    report: PathBuf,
}

impl Cli {
    fn parse() -> Result<Self> {
        let mut args = env::args_os().skip(1);
        let mut serial = env::var("ANDROID_SERIAL").ok();
        let mut adb = env::var_os("ANDROID_HOME")
            .map(PathBuf::from)
            .map(|root| root.join("platform-tools/adb"))
            .unwrap_or_else(|| PathBuf::from("adb"));
        let mut driver = PathBuf::from("/data/main/cargo-target/egui-tester-android/driver.apk");
        let mut score = Score::Comprehensive;
        let mut fresh = false;
        let mut captures = None;
        let mut profile = None;
        let mut trace_processor = env::var_os("TRACE_PROCESSOR")
            .map_or_else(|| PathBuf::from("trace_processor"), PathBuf::from);
        while let Some(argument) = args.next() {
            match argument.to_str() {
                Some("--serial") => serial = Some(text(required(&mut args, "--serial")?)?),
                Some("--adb") => adb = required(&mut args, "--adb")?.into(),
                Some("--driver") => driver = required(&mut args, "--driver")?.into(),
                Some("--trace-processor") => {
                    trace_processor = required(&mut args, "--trace-processor")?.into();
                }
                Some("--score") => {
                    score = Score::parse(&text(required(&mut args, "--score")?)?)?;
                }
                Some("--fresh") => fresh = true,
                Some("--captures") => {
                    captures = Some(required(&mut args, "--captures DIRECTORY")?.into());
                }
                Some("--profile") => {
                    profile = Some(Profile {
                        config: required(&mut args, "--profile CONFIG TRACE EVENTS REPORT")?.into(),
                        trace: required(&mut args, "--profile CONFIG TRACE EVENTS REPORT")?.into(),
                        events: required(&mut args, "--profile CONFIG TRACE EVENTS REPORT")?.into(),
                        report: required(&mut args, "--profile CONFIG TRACE EVENTS REPORT")?.into(),
                    });
                }
                Some(flag) => return Err(verdict(format!("unknown option `{flag}`"))),
                None => return Err(verdict("options must be valid Unicode")),
            }
        }
        Ok(Self {
            serial: serial.ok_or_else(|| verdict("--serial or ANDROID_SERIAL is required"))?,
            adb,
            driver,
            score,
            fresh,
            captures,
            profile,
            trace_processor,
        })
    }
}

fn required(args: &mut impl Iterator<Item = OsString>, flag: &str) -> Result<OsString> {
    args.next()
        .ok_or_else(|| verdict(format!("{flag} requires another value")))
}

fn text(value: OsString) -> Result<String> {
    value
        .into_string()
        .map_err(|_| verdict("option value must be valid Unicode"))
}

fn verdict(detail: impl Into<String>) -> Error {
    Error::Verdict {
        detail: detail.into(),
    }
}
