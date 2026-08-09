use crate::{
    app::WeatherApp,
    map::MapGpu,
    tray::{Signal as TraySignal, Tray},
    vector_map::VectorMapGpu,
};
use anyhow::Result;
use eternalist_apps::{CloseDisposition, NativeApp, WindowSpec};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

const TITLE: &str = "HRRR · native forecast fields";

pub fn run(ctx: egui::Context) -> Result<()> {
    let app = ForecastViewer {
        weather: WeatherApp::open(&ctx)?,
        tray: None,
        tray_armed: false,
        quit: Arc::new(AtomicBool::new(false)),
    };
    eternalist_apps::run(ctx, app)
}

struct ForecastViewer {
    weather: WeatherApp,
    tray: Option<Tray>,
    tray_armed: bool,
    quit: Arc<AtomicBool>,
}

impl ForecastViewer {
    fn arm_tray(&mut self, ctx: &egui::Context) {
        if self.tray_armed {
            return;
        }
        self.tray_armed = true;
        let ctx = ctx.clone();
        let quit = Arc::clone(&self.quit);
        match Tray::raise(move |signal| match signal {
            TraySignal::Reveal => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                ctx.request_repaint();
            }
            TraySignal::Quit => {
                quit.store(true, Ordering::Release);
                ctx.request_repaint();
            }
        }) {
            Ok(tray) => self.tray = Some(tray),
            Err(error) => eprintln!("could not raise HRRR tray icon: {error:#}"),
        }
    }
}

impl NativeApp for ForecastViewer {
    const WINDOW: WindowSpec = WindowSpec::new(TITLE, [1_440.0, 920.0]);

    fn draw(&mut self, ui: &mut egui::Ui) {
        self.arm_tray(ui.ctx());
        self.weather.pulse(ui);
    }

    fn close_requested(&mut self) -> CloseDisposition {
        self.weather.retire();
        if self.weather.close_minimizes() && self.tray.as_ref().is_some_and(Tray::available) {
            CloseDisposition::Hide
        } else {
            CloseDisposition::Exit
        }
    }

    fn exit_requested(&self) -> bool {
        self.quit.load(Ordering::Acquire)
    }

    fn after_present(&mut self) -> bool {
        false
    }

    fn water(
        &mut self,
        ctx: &egui::Context,
        pixels_per_point: f32,
        tooltip_rects: &[egui::Rect],
    ) -> dwemer_poolrooms::water::Frame {
        self.weather
            .water_frame(ctx, pixels_per_point, tooltip_rects)
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
    type Observation = crate::witness::State;

    #[cfg(feature = "egui-test")]
    fn observe(&self, _text_edit_focused: bool) -> Self::Observation {
        self.weather.witness_state()
    }
}
