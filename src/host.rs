use crate::{
    app::WeatherApp,
    application_paths::{ApplicationPaths, InstanceGuard},
    basemap_artifact::{self, InstallPhase, InstallProgress},
    map::MapGpu,
    tray::{Signal as TraySignal, Tray},
    vector_map::VectorMapGpu,
    witness,
};
use anyhow::{Context as _, Result};
use brass_poolrooms::{
    chrome,
    water::{Frame as WaterFrame, Surface, Wetness},
};
use crossbeam_channel::{Receiver, bounded};
use eternalist_apps::{
    CloseDisposition, CrashProduct, CrashReportSpec, LivingWait, NativeApp, NativeWake, WindowSpec,
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Instant,
};

const TITLE: &str = "HRRR";

pub fn run(ctx: egui::Context) -> Result<()> {
    eternalist_apps::run_with(ctx, ForecastViewer::open)
}

struct ForecastViewer {
    body: Body,
    tray: Option<Tray>,
    tray_armed: bool,
    reveal: Arc<AtomicBool>,
    quit: Arc<AtomicBool>,
    wait: LivingWait,
    launch_water: Surface,
}

impl ForecastViewer {
    fn open(ctx: &egui::Context) -> Result<Self> {
        Ok(Self {
            body: Body::open(ctx)?,
            tray: None,
            tray_armed: false,
            reveal: Arc::new(AtomicBool::new(false)),
            quit: Arc::new(AtomicBool::new(false)),
            wait: LivingWait::default(),
            launch_water: Surface::new(Wetness::Wet),
        })
    }

    fn arm_tray(&mut self, ctx: &egui::Context) {
        if self.tray_armed || !matches!(self.body, Body::Ready(_)) {
            return;
        }
        self.tray_armed = true;
        let wake = NativeWake::from_context(ctx);
        let reveal = Arc::clone(&self.reveal);
        let quit = Arc::clone(&self.quit);
        match Tray::raise(move |signal| match signal {
            TraySignal::Reveal => {
                reveal.store(true, Ordering::Release);
                let _woken = wake.wake();
            }
            TraySignal::Quit => {
                quit.store(true, Ordering::Release);
                let _woken = wake.wake();
            }
        }) {
            Ok(tray) => self.tray = Some(tray),
            Err(error) => eprintln!("could not raise HRRR tray icon: {error:#}"),
        }
    }

    fn draw_body(&mut self, ui: &mut egui::Ui) {
        let event = self.body.draw(ui, &mut self.wait, &mut self.launch_water);
        let Some(event) = event else {
            return;
        };
        let body = std::mem::replace(&mut self.body, Body::Poisoned);
        self.body = body.transition(event, ui.ctx());
    }
}

impl NativeApp for ForecastViewer {
    const WINDOW: WindowSpec = WindowSpec::new(TITLE, [1_440.0, 920.0]);

    fn crash_reports() -> Option<CrashReportSpec> {
        ApplicationPaths::claim().ok().map(|paths| {
            CrashReportSpec::new(CrashProduct::Hrrr, env!("CARGO_PKG_VERSION"), paths.state)
        })
    }

    fn draw(&mut self, ui: &mut egui::Ui) {
        self.arm_tray(ui.ctx());
        self.draw_body(ui);
    }

    fn close_requested(&mut self) -> CloseDisposition {
        match &mut self.body {
            Body::Ready(weather) => {
                if weather.close_to_tray_enabled()
                    && self.tray.as_ref().is_some_and(Tray::available)
                {
                    CloseDisposition::HideOrExit
                } else {
                    CloseDisposition::Exit
                }
            }
            body => {
                body.cancel();
                CloseDisposition::Exit
            }
        }
    }

    fn exit_requested(&self) -> bool {
        self.quit.load(Ordering::Acquire)
    }

    fn take_reveal_request(&mut self) -> bool {
        self.reveal.swap(false, Ordering::AcqRel)
    }

    fn service_deadline(&self, now: Instant) -> Option<Instant> {
        match &self.body {
            Body::Ready(weather) => weather.service_deadline(now),
            _ => None,
        }
    }

    fn service_deadline_reached(&mut self, now: Instant) -> bool {
        match &mut self.body {
            Body::Ready(weather) => weather.service_deadline_reached(now),
            _ => false,
        }
    }

    fn after_present(&mut self) -> bool {
        false
    }

    fn water(
        &mut self,
        ctx: &egui::Context,
        pixels_per_point: f32,
        tooltip_rects: &[egui::Rect],
    ) -> WaterFrame {
        if let Body::Ready(weather) = &mut self.body {
            weather.water_frame(ctx, pixels_per_point, tooltip_rects)
        } else {
            self.wait.compose(ctx, &mut self.launch_water);
            self.launch_water
                .frame(ctx, pixels_per_point, tooltip_rects, None)
        }
    }

    fn register_gpu(
        renderer: &mut egui_wgpu::Renderer,
        device: &egui_wgpu::wgpu::Device,
        format: egui_wgpu::wgpu::TextureFormat,
    ) {
        let _prior = renderer
            .callback_resources
            .insert(MapGpu::new(device, format));
        let _prior = renderer
            .callback_resources
            .insert(VectorMapGpu::new(device, format));
    }

    #[cfg(feature = "egui-test")]
    type Observation = witness::State;

    #[cfg(feature = "egui-test")]
    fn observe(&self, _text_edit_focused: bool) -> Self::Observation {
        match &self.body {
            Body::Ready(weather) => weather.witness_state(),
            body => witness::State::threshold(body.witness_phase()),
        }
    }
}

struct Seed {
    paths: ApplicationPaths,
    instance: InstanceGuard,
}

impl Seed {
    fn claim() -> Result<Self> {
        let paths = ApplicationPaths::claim()?;
        let instance = paths.lock_instance()?;
        Ok(Self { paths, instance })
    }

    fn archive(&self) -> Result<PathBuf> {
        self.paths.basemap_path()
    }

    fn open(self, ctx: &egui::Context) -> Result<WeatherApp> {
        WeatherApp::open_at(ctx, self.paths, self.instance)
    }
}

enum Body {
    Consent(Seed),
    Installing { seed: Seed, worker: Installer },
    Fault(Fault),
    Ready(Box<WeatherApp>),
    Poisoned,
}

impl Body {
    fn open(ctx: &egui::Context) -> Result<Self> {
        let seed = Seed::claim()?;
        let archive = seed.archive()?;
        if archive.is_file() {
            return Ok(Self::Ready(Box::new(seed.open(ctx)?)));
        }
        if ApplicationPaths::basemap_is_external() {
            return Ok(Self::Fault(Fault {
                seed: Some(seed),
                title: "CONFIGURED BASEMAP NOT FOUND".to_owned(),
                detail: format!("No archive exists at {}.", archive.display()),
                action: FaultAction::Recheck,
            }));
        }
        Ok(Self::Consent(seed))
    }

    fn draw(
        &mut self,
        ui: &mut egui::Ui,
        wait: &mut LivingWait,
        water: &mut Surface,
    ) -> Option<BodyEvent> {
        match self {
            Self::Consent(_) => consent(ui, water),
            Self::Installing { worker, .. } => {
                let settled = worker.poll();
                let action = installing(ui, wait, water, worker);
                settled.map(BodyEvent::Settled).or(action)
            }
            Self::Fault(fault) => fault.draw(ui, water),
            Self::Ready(weather) => {
                weather.pulse(ui);
                None
            }
            Self::Poisoned => unreachable!("HRRR launch state escaped a transition"),
        }
    }

    fn transition(self, event: BodyEvent, ctx: &egui::Context) -> Self {
        match (self, event) {
            (Self::Consent(seed), BodyEvent::Install)
            | (
                Self::Fault(Fault {
                    seed: Some(seed),
                    action: FaultAction::RetryInstall,
                    ..
                }),
                BodyEvent::Install,
            ) => match Installer::spawn(&seed.paths, ctx) {
                Ok(worker) => Self::Installing { seed, worker },
                Err(error) => Self::install_fault(seed, error),
            },
            (Self::Installing { seed, mut worker }, BodyEvent::Cancel) => {
                worker.cancel();
                Self::Installing { seed, worker }
            }
            (Self::Installing { seed, .. }, BodyEvent::Settled(Ok(_archive))) => {
                match seed.open(ctx) {
                    Ok(weather) => Self::Ready(Box::new(weather)),
                    Err(error) => Self::Fault(Fault {
                        seed: None,
                        title: "BASEMAP INSTALLED; STARTUP FAILED".to_owned(),
                        detail: format!("{error:#}"),
                        action: FaultAction::None,
                    }),
                }
            }
            (Self::Installing { seed, .. }, BodyEvent::Settled(Err(error)))
                if basemap_artifact::was_cancelled(&error) =>
            {
                Self::Consent(seed)
            }
            (Self::Installing { seed, .. }, BodyEvent::Settled(Err(error))) => {
                Self::install_fault(seed, error)
            }
            (
                Self::Fault(Fault {
                    seed: Some(seed),
                    action: FaultAction::Recheck,
                    ..
                }),
                BodyEvent::Recheck,
            ) => match seed.archive() {
                Ok(archive) if archive.is_file() => match seed.open(ctx) {
                    Ok(weather) => Self::Ready(Box::new(weather)),
                    Err(error) => Self::Fault(Fault {
                        seed: None,
                        title: "BASEMAP OPEN FAILED".to_owned(),
                        detail: format!("{error:#}"),
                        action: FaultAction::None,
                    }),
                },
                Ok(archive) => Self::Fault(Fault {
                    seed: Some(seed),
                    title: "CONFIGURED BASEMAP NOT FOUND".to_owned(),
                    detail: format!("No archive exists at {}.", archive.display()),
                    action: FaultAction::Recheck,
                }),
                Err(error) => Self::Fault(Fault {
                    seed: Some(seed),
                    title: "BASEMAP PATH REJECTED".to_owned(),
                    detail: format!("{error:#}"),
                    action: FaultAction::Recheck,
                }),
            },
            (body, _) => {
                debug_assert!(false, "illegal HRRR launch transition");
                body
            }
        }
    }

    fn install_fault(seed: Seed, error: anyhow::Error) -> Self {
        Self::Fault(Fault {
            seed: Some(seed),
            title: "BASEMAP INSTALL FAILED".to_owned(),
            detail: format!("{error:#}"),
            action: FaultAction::RetryInstall,
        })
    }

    fn cancel(&mut self) {
        if let Self::Installing { worker, .. } = self {
            worker.cancel();
        }
    }

    #[cfg(feature = "egui-test")]
    const fn witness_phase(&self) -> &'static str {
        match self {
            Self::Consent(_) => "basemap-required",
            Self::Installing { .. } => "basemap-installing",
            Self::Fault(_) => "basemap-fault",
            Self::Ready(_) => "ready",
            Self::Poisoned => "transitioning",
        }
    }
}

enum BodyEvent {
    Install,
    Cancel,
    Recheck,
    Settled(Result<PathBuf>),
}

struct Fault {
    seed: Option<Seed>,
    title: String,
    detail: String,
    action: FaultAction,
}

struct LaunchAction {
    label: &'static str,
    event: BodyEvent,
    target: Option<hrrr_contract::Target>,
}

#[derive(Clone, Copy)]
enum FaultAction {
    RetryInstall,
    Recheck,
    None,
}

impl Fault {
    fn draw(&self, ui: &mut egui::Ui, water: &mut Surface) -> Option<BodyEvent> {
        let action = match self.action {
            FaultAction::RetryInstall => Some(LaunchAction {
                label: "TRY AGAIN",
                event: BodyEvent::Install,
                target: None,
            }),
            FaultAction::Recheck => Some(LaunchAction {
                label: "RECHECK",
                event: BodyEvent::Recheck,
                target: None,
            }),
            FaultAction::None => None,
        };
        launch_card(ui, water, &self.title, &self.detail, action).0
    }
}

fn consent(ui: &mut egui::Ui, water: &mut Surface) -> Option<BodyEvent> {
    let detail = "Download a North America map for fast browsing through zoom 11. It uses about 1.1 GB of network and disk. Closer detail downloads as viewed and stays in a bounded cache.";
    launch_card(
        ui,
        water,
        "MAP DOWNLOAD REQUIRED",
        detail,
        Some(LaunchAction {
            label: "DOWNLOAD MAP",
            event: BodyEvent::Install,
            target: Some(hrrr_contract::Target::BasemapInstall),
        }),
    )
    .0
}

fn installing(
    ui: &mut egui::Ui,
    wait: &mut LivingWait,
    water: &mut Surface,
    worker: &Installer,
) -> Option<BodyEvent> {
    let detail = if worker.cancelling {
        "CANCELING…".to_owned()
    } else {
        format!(
            "{}\n{}",
            worker.progress.phase.label(),
            progress_bytes(worker.progress)
        )
    };
    let action = (!worker.cancelling).then_some(LaunchAction {
        label: "CANCEL",
        event: BodyEvent::Cancel,
        target: None,
    });
    let (event, rect) = launch_card(ui, water, "PREPARING MAP", &detail, action);
    wait.claim(rect);
    event
}

fn launch_card(
    ui: &mut egui::Ui,
    water: &mut Surface,
    title: &str,
    detail: &str,
    action: Option<LaunchAction>,
) -> (Option<BodyEvent>, egui::Rect) {
    let mut event = None;
    let panel = egui::CentralPanel::default().show(ui, |ui| {
        let top = ((ui.available_height() - 250.0) * 0.5).max(24.0);
        ui.add_space(top);
        let centered = ui.vertical_centered(|ui| {
            ui.set_max_width(560.0);
            let card = egui::Frame::new()
                .fill(chrome::SURFACE)
                .stroke(egui::Stroke::new(1.0_f32, chrome::EDGE_STRONG))
                .corner_radius(2)
                .inner_margin(egui::Margin::symmetric(28, 24))
                .show(ui, |ui| {
                    ui.set_min_width(500.0_f32.min(ui.available_width()));
                    let _title = ui.label(chrome::title(title));
                    ui.add_space(10.0);
                    let _detail = ui.label(chrome::muted(detail));
                    if let Some(action) = action {
                        ui.add_space(18.0);
                        let response = ui.add_sized([220.0, 34.0], egui::Button::new(action.label));
                        chrome::tension(ui, &response);
                        if let Some(target) = action.target {
                            witness::anchor(ui, target, response.rect);
                        }
                        if response.clicked() {
                            water.click(response.rect);
                            event = Some(action.event);
                        }
                    }
                });
            card.response.rect
        });
        centered.inner
    });
    (event, panel.inner)
}

impl InstallPhase {
    const fn label(self) -> &'static str {
        match self {
            Self::FetchingTool => "DOWNLOADING MAP TOOLS",
            Self::CheckingTool => "VERIFYING MAP TOOLS",
            Self::UnpackingTool => "PREPARING MAP TOOLS",
            Self::ExtractingMap => "BUILDING NORTH AMERICA MAP",
            Self::CheckingMap => "VERIFYING MAP",
        }
    }
}

fn progress_bytes(progress: InstallProgress) -> String {
    if progress.bytes == 0 {
        return "Preparing…".to_owned();
    }
    let bytes = decimal_bytes(progress.bytes);
    progress.total.map_or_else(
        || format!("{bytes} written"),
        |total| format!("{bytes} / {}", decimal_bytes(total)),
    )
}

fn decimal_bytes(bytes: u64) -> String {
    const KB: f64 = 1_000.0;
    const MB: f64 = 1_000_000.0;
    const GB: f64 = 1_000_000_000.0;
    if bytes >= 1_000_000_000 {
        format!("{:.2} GB", bytes as f64 / GB)
    } else if bytes >= 1_000_000 {
        format!("{:.0} MB", bytes as f64 / MB)
    } else {
        format!("{:.0} kB", bytes as f64 / KB)
    }
}

struct Installer {
    cancel: Arc<AtomicBool>,
    events: Receiver<InstallEvent>,
    thread: Option<JoinHandle<()>>,
    progress: InstallProgress,
    cancelling: bool,
}

enum InstallEvent {
    Progress(InstallProgress),
    Settled(Result<PathBuf>),
}

impl Installer {
    fn spawn(paths: &ApplicationPaths, ctx: &egui::Context) -> Result<Self> {
        let cancel = Arc::new(AtomicBool::new(false));
        let thread_cancel = Arc::clone(&cancel);
        let paths = paths.clone();
        let wake = NativeWake::from_context(ctx);
        let (send, events) = bounded(1);
        let progress_send = send.clone();
        let thread = thread::Builder::new()
            .name("hrrr-basemap-install".to_owned())
            .spawn(move || {
                let result =
                    basemap_artifact::install_attended(&paths, None, &thread_cancel, |progress| {
                        if progress_send
                            .try_send(InstallEvent::Progress(progress))
                            .is_ok()
                        {
                            let _woken = wake.request_foreground_repaint();
                        }
                    });
                let _sent = send.send(InstallEvent::Settled(result));
                let _woken = wake.request_foreground_repaint();
            })
            .context("spawn basemap installer")?;
        Ok(Self {
            cancel,
            events,
            thread: Some(thread),
            progress: InstallProgress {
                phase: InstallPhase::FetchingTool,
                bytes: 0,
                total: None,
            },
            cancelling: false,
        })
    }

    fn poll(&mut self) -> Option<Result<PathBuf>> {
        let mut settled = None;
        while let Ok(event) = self.events.try_recv() {
            match event {
                InstallEvent::Progress(progress) => self.progress = progress,
                InstallEvent::Settled(result) => settled = Some(result),
            }
        }
        if settled.is_some()
            && let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            return Some(Err(anyhow::anyhow!("basemap installer thread panicked")));
        }
        settled
    }

    fn cancel(&mut self) {
        self.cancelling = true;
        self.cancel.store(true, Ordering::Release);
    }
}

impl Drop for Installer {
    fn drop(&mut self) {
        self.cancel();
    }
}
