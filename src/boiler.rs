//! Bespoke winit + wgpu + egui integration. Poolrooms' water owns the final
//! composite, while the HRRR field enters egui's pass as a native GPU callback.

use crate::{
    app::WeatherApp,
    map::MapGpu,
    tray::{Signal as TraySignal, Tray},
    vector_map::VectorMapGpu,
};
use anyhow::{Context as _, Result};
use dwemer_poolrooms::water::Engine;
use egui_wgpu::{RenderState, RendererOptions, ScreenDescriptor, WgpuConfiguration, wgpu};
use egui_winit::winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalSize},
    event::{StartCause, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    window::{Window, WindowAttributes},
};
use std::{
    sync::{Arc, Mutex, MutexGuard},
    time::Instant,
};

const WINDOW_SIZE: LogicalSize<f64> = LogicalSize::new(1440.0, 920.0);

#[derive(Clone, Copy, Debug)]
enum Spark {
    Repaint,
    Tray(TraySignal),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CloseDisposition {
    Hide,
    Exit,
}

const fn close_disposition(close_minimizes: bool, tray_available: bool) -> CloseDisposition {
    if close_minimizes && tray_available {
        CloseDisposition::Hide
    } else {
        CloseDisposition::Exit
    }
}

type Alarm = Arc<Mutex<Option<Instant>>>;

pub fn run(ctx: egui::Context) -> Result<()> {
    let event_loop = EventLoop::<Spark>::with_user_event()
        .build()
        .context("build event loop")?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let alarm = Alarm::default();
    let proxy = event_loop.create_proxy();
    arm_repaints(&ctx, alarm.clone(), proxy.clone());
    let mut boiler = Boiler {
        ctx,
        app: None,
        alarm,
        proxy,
        rig: None,
        tray: None,
        fatal: None,
    };
    event_loop.run_app(&mut boiler).context("run event loop")?;
    boiler.fatal.map_or(Ok(()), Err)
}

fn arm_repaints(ctx: &egui::Context, alarm: Alarm, proxy: EventLoopProxy<Spark>) {
    ctx.set_request_repaint_callback(move |info| {
        advance_alarm(&alarm, Instant::now() + info.delay);
        let _woken = proxy.send_event(Spark::Repaint);
    });
}

fn advance_alarm(alarm: &Alarm, when: Instant) {
    let mut alarm = lock_alarm(alarm);
    if alarm.is_none_or(|set| when < set) {
        *alarm = Some(when);
    }
}

fn lock_alarm(alarm: &Alarm) -> MutexGuard<'_, Option<Instant>> {
    match alarm.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

struct Boiler {
    ctx: egui::Context,
    app: Option<WeatherApp>,
    alarm: Alarm,
    proxy: EventLoopProxy<Spark>,
    rig: Option<Rig>,
    tray: Option<Tray>,
    fatal: Option<anyhow::Error>,
}

impl Boiler {
    fn paint(&mut self) {
        let Some(rig) = self.rig.as_mut() else {
            return;
        };
        let Some(app) = self.app.as_mut() else {
            return;
        };
        let raw_input = rig.input.take_egui_input(&rig.window);
        let output = self.ctx.run_ui(raw_input, |ui| app.pulse(ui));
        rig.input
            .handle_platform_output(&rig.window, output.platform_output);
        let primitives = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        let tooltip_rects = tooltip_rects(&self.ctx);
        let water = app.water_frame(&self.ctx, output.pixels_per_point, &tooltip_rects);
        if water.wants_repaint() {
            rig.window.request_redraw();
        }
        rig.render(
            &primitives,
            &output.textures_delta,
            output.pixels_per_point,
            &water,
        );
        if let Some(viewport) = output.viewport_output.get(&egui::ViewportId::ROOT) {
            if viewport.repaint_delay.is_zero() {
                rig.window.request_redraw();
            } else if let Some(when) = Instant::now().checked_add(viewport.repaint_delay) {
                advance_alarm(&self.alarm, when);
            }
        }
    }

    fn tend_alarm(&self) {
        let Some(rig) = &self.rig else {
            return;
        };
        let mut alarm = lock_alarm(&self.alarm);
        if alarm.is_some_and(|when| when <= Instant::now()) {
            *alarm = None;
            rig.window.request_redraw();
        }
    }
}

fn tooltip_rects(ctx: &egui::Context) -> Vec<egui::Rect> {
    ctx.memory(|memory| {
        memory
            .layer_ids()
            .filter(|layer| layer.order == egui::Order::Tooltip && memory.areas().is_visible(layer))
            .filter_map(|layer| memory.area_rect(layer.id))
            .filter(egui::Rect::is_positive)
            .collect()
    })
}

impl ApplicationHandler<Spark> for Boiler {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.rig.is_some() {
            return;
        }
        match Rig::raise(event_loop, &self.ctx) {
            Ok(rig) => {
                self.rig = Some(rig);
                match WeatherApp::open(&self.ctx) {
                    Ok(app) => self.app = Some(app),
                    Err(error) => {
                        self.fatal = Some(error.context("open HRRR application"));
                        event_loop.exit();
                        return;
                    }
                }
                let proxy = self.proxy.clone();
                match Tray::raise(move |signal| {
                    let _delivered = proxy.send_event(Spark::Tray(signal));
                }) {
                    Ok(tray) => self.tray = Some(tray),
                    Err(err) => eprintln!("could not raise HRRR tray icon: {err:#}"),
                }
            }
            Err(err) => {
                self.fatal = Some(err.context("raise HRRR window"));
                event_loop.exit();
            }
        }
    }

    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: StartCause) {
        if matches!(cause, StartCause::ResumeTimeReached { .. }) {
            self.tend_alarm();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Spark) {
        match event {
            Spark::Repaint => self.tend_alarm(),
            Spark::Tray(TraySignal::Reveal) => {
                if let Some(rig) = &self.rig {
                    rig.window.set_visible(true);
                    rig.window.set_minimized(false);
                    rig.window.focus_window();
                    rig.window.request_redraw();
                }
            }
            Spark::Tray(TraySignal::Quit) => {
                if let Some(app) = &mut self.app {
                    app.retire();
                }
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: egui_winit::winit::window::WindowId,
        event: WindowEvent,
    ) {
        match &event {
            WindowEvent::CloseRequested => {
                let Some(app) = &mut self.app else {
                    event_loop.exit();
                    return;
                };
                app.retire();
                let tray_available = self.tray.as_ref().is_some_and(Tray::available);
                match close_disposition(app.close_minimizes(), tray_available) {
                    CloseDisposition::Hide => {
                        if let Some(rig) = &self.rig {
                            rig.window.set_visible(false);
                        }
                    }
                    CloseDisposition::Exit => event_loop.exit(),
                }
                return;
            }
            WindowEvent::RedrawRequested => {
                self.paint();
                return;
            }
            WindowEvent::Resized(size) => {
                if let Some(rig) = &mut self.rig {
                    rig.resize(*size);
                }
            }
            _ => {}
        }
        let Some(rig) = &mut self.rig else {
            return;
        };
        let response = rig.input.on_window_event(&rig.window, &event);
        if response.repaint {
            rig.window.request_redraw();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.tend_alarm();
        let deadline = *lock_alarm(&self.alarm);
        event_loop.set_control_flow(match deadline {
            Some(when) => ControlFlow::WaitUntil(when),
            None => ControlFlow::Wait,
        });
    }
}

struct Rig {
    window: Arc<Window>,
    input: egui_winit::State,
    surface: wgpu::Surface<'static>,
    gpu: RenderState,
    config: wgpu::SurfaceConfiguration,
    water_engine: Engine,
}

impl Rig {
    fn raise(event_loop: &ActiveEventLoop, ctx: &egui::Context) -> Result<Self> {
        let window = Arc::new(
            event_loop
                .create_window(
                    WindowAttributes::default()
                        .with_title("HRRR · native forecast fields")
                        .with_inner_size(WINDOW_SIZE),
                )
                .context("create window")?,
        );
        let input = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            window.theme(),
            None,
        );
        let configuration = WgpuConfiguration::default();
        let instance = pollster::block_on(configuration.wgpu_setup.new_instance());
        let surface = instance
            .create_surface(window.clone())
            .context("create surface")?;
        let gpu = pollster::block_on(RenderState::create(
            &configuration,
            &instance,
            Some(&surface),
            RendererOptions::default(),
        ))
        .context("create wgpu render state")?;
        {
            let mut renderer = gpu.renderer.write();
            let _prior = renderer
                .callback_resources
                .insert(MapGpu::new(&gpu.device, gpu.target_format));
            let _prior = renderer
                .callback_resources
                .insert(VectorMapGpu::new(&gpu.device, gpu.target_format));
        }
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&gpu.adapter, size.width.max(1), size.height.max(1))
            .context("surface is unsupported by the adapter")?;
        config.format = gpu.target_format;
        config.present_mode = wgpu::PresentMode::AutoVsync;
        config.view_formats = vec![gpu.target_format];
        surface.configure(&gpu.device, &config);
        let mut water_engine = Engine::new(&gpu.device, gpu.target_format);
        water_engine.resize(&gpu.device, config.width, config.height);
        Ok(Self {
            window,
            input,
            surface,
            gpu,
            config,
            water_engine,
        })
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.gpu.device, &self.config);
        self.water_engine
            .resize(&self.gpu.device, size.width, size.height);
    }

    fn render(
        &mut self,
        primitives: &[egui::ClippedPrimitive],
        delta: &egui::TexturesDelta,
        pixels_per_point: f32,
        water: &dwemer_poolrooms::water::Frame,
    ) {
        let screen = ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point,
        };
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("hrrr-boiler"),
            });
        let user_commands = {
            let mut renderer = self.gpu.renderer.write();
            for (id, image_delta) in &delta.set {
                renderer.update_texture(&self.gpu.device, &self.gpu.queue, *id, image_delta);
            }
            renderer.update_buffers(
                &self.gpu.device,
                &self.gpu.queue,
                &mut encoder,
                primitives,
                &screen,
            )
        };
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Timeout => {
                self.window.request_redraw();
                return;
            }
            wgpu::CurrentSurfaceTexture::Occluded => return,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.gpu.device, &self.config);
                self.window.request_redraw();
                return;
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                eprintln!("surface texture validation failure");
                return;
            }
        };
        let surface_view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        if water.dry() {
            self.water_engine.becalm(&self.gpu.queue);
        }
        let frosted = water.live() && self.water_engine.scene_view().is_some();
        {
            let target = if frosted {
                self.water_engine.scene_view().unwrap_or(&surface_view)
            } else {
                &surface_view
            };
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("hrrr-egui"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                })
                .forget_lifetime();
            self.gpu
                .renderer
                .read()
                .render(&mut pass, primitives, &screen);
        }
        if frosted {
            self.water_engine.compose(
                &self.gpu.device,
                &self.gpu.queue,
                &mut encoder,
                &surface_view,
                water,
            );
        }
        let _submission = self
            .gpu
            .queue
            .submit(user_commands.into_iter().chain([encoder.finish()]));
        if self
            .water_engine
            .after_submit(&self.gpu.device, &self.gpu.queue, water)
        {
            self.window.request_redraw();
        }
        self.window.pre_present_notify();
        frame.present();
        let mut renderer = self.gpu.renderer.write();
        for id in &delta.free {
            renderer.free_texture(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_hides_only_under_both_preference_and_tray_capability() {
        assert_eq!(close_disposition(true, true), CloseDisposition::Hide);
        assert_eq!(close_disposition(false, true), CloseDisposition::Exit);
        assert_eq!(close_disposition(true, false), CloseDisposition::Exit);
        assert_eq!(close_disposition(false, false), CloseDisposition::Exit);
    }
}
