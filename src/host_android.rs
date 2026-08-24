//! Android `NativeActivity` boundary.

use crate::{
    app::WeatherApp, application_paths::ApplicationPaths, map::MapGpu, vector_map::VectorMapGpu,
};
use anyhow::Result;
use brass_poolrooms::water::Frame as WaterFrame;
use eternalist_apps::{AndroidApp, NativeApp, WindowSpec};
use std::time::Instant;

const TITLE: &str = "HRRR";

pub fn run(android: AndroidApp, ctx: egui::Context) -> Result<()> {
    eternalist_apps::run_android_with(android, ctx, ForecastViewer::open)
}

struct ForecastViewer {
    weather: WeatherApp,
}

impl ForecastViewer {
    fn open(ctx: &egui::Context, android: &AndroidApp) -> Result<Self> {
        let paths = ApplicationPaths::claim_android(android)?;
        let instance = paths.lock_instance()?;
        Ok(Self {
            weather: WeatherApp::open_at(ctx, paths, instance)?,
        })
    }
}

impl NativeApp for ForecastViewer {
    const WINDOW: WindowSpec = WindowSpec::new(TITLE, [480.0, 800.0]);

    fn draw(&mut self, ui: &mut egui::Ui) {
        self.weather.pulse(ui);
    }

    fn service_deadline(&self, now: Instant) -> Option<Instant> {
        self.weather.service_deadline(now)
    }

    fn service_deadline_reached(&mut self, now: Instant) -> bool {
        self.weather.service_deadline_reached(now)
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

    fn suspended(&mut self) {
        self.weather.suspend();
    }
}
