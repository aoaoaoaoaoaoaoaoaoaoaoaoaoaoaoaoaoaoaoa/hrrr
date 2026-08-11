use crate::{
    model::{FieldGrid, FrameKey, LambertGrid, Viewport},
    spec::Scale,
};
use bytemuck::{Pod, Zeroable};
use egui_wgpu::{CallbackResources, CallbackTrait, ScreenDescriptor, wgpu};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

const TILE_EDGE: f64 = 256.0;
const EARTH_CIRCUMFERENCE_M: f64 = 40_075_016.686;
const VISIBLE_SAMPLE_PITCH: f32 = 16.0;
/// A North American overview: about 59°N to 9°N at the default center.
const MAX_VERTICAL_WORLD_SPAN: f64 = 0.18;
const SCALE_CAPACITY: usize = 128;

#[derive(Debug, Default)]
pub struct ScaleBar {
    current_m: Option<f64>,
    born: Option<Instant>,
    departing: Option<(f64, Instant)>,
}

impl ScaleBar {
    pub fn paint(&mut self, painter: &egui::Painter, view: Viewport, rect: egui::Rect) {
        let meters_per_point = ground_meters_per_point(view);
        let target = pleasant_length(meters_per_point * 105.0);
        let current = self.current_m.get_or_insert(target);
        let current_width = *current / meters_per_point;
        if current.to_bits() != target.to_bits() && !(72.0..=148.0).contains(&current_width) {
            self.departing = Some((*current, Instant::now()));
            *current = target;
            self.born = Some(Instant::now());
        }

        let now = Instant::now();
        if let Some((departing, begun)) = self.departing {
            let maturity = smooth_transition(now.saturating_duration_since(begun));
            paint_scale_length(painter, rect, departing, meters_per_point, 1.0 - maturity);
            if maturity >= 1.0 {
                self.departing = None;
            } else {
                painter.ctx().request_repaint();
            }
        }
        let maturity = self.born.map_or(1.0, |begun| {
            smooth_transition(now.saturating_duration_since(begun))
        });
        paint_scale_length(painter, rect, *current, meters_per_point, maturity);
        if maturity < 1.0 {
            painter.ctx().request_repaint();
        } else {
            self.born = None;
        }
    }
}

fn smooth_transition(elapsed: Duration) -> f32 {
    let phase = (elapsed.as_secs_f32() / 0.16).clamp(0.0, 1.0);
    phase * phase * 2.0_f32.mul_add(-phase, 3.0)
}

#[derive(Clone)]
pub struct FieldPaint {
    pub key: FrameKey,
    pub field: Arc<FieldGrid>,
    pub scale: Scale,
    pub world_bounds: [f32; 4],
}

impl CallbackTrait for FieldPaint {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(gpu) = resources.get_mut::<MapGpu>() {
            gpu.prepare(device, queue, self);
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        if let Some(gpu) = resources.get::<MapGpu>() {
            pass.set_pipeline(&gpu.pipeline);
            pass.set_bind_group(0, &gpu.bind, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

pub struct MapGpu {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    texture: wgpu::Texture,
    bind: wgpu::BindGroup,
    resident: Option<FrameKey>,
    extent: [u32; 2],
}

impl MapGpu {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hrrr-field"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(size_of::<Uniform>() as u64),
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hrrr-field"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hrrr-field"),
            source: wgpu::ShaderSource::Wgsl(WGSL.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hrrr-field"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hrrr-field-uniform"),
            size: size_of::<Uniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let texture = field_texture(device, [1, 1]);
        let bind = field_bind(device, &layout, &texture, &uniform);
        Self {
            pipeline,
            layout,
            uniform,
            texture,
            bind,
            resident: None,
            extent: [1, 1],
        }
    }

    fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, paint: &FieldPaint) {
        let extent = [paint.field.width, paint.field.height];
        if self.extent != extent {
            self.texture = field_texture(device, extent);
            self.bind = field_bind(device, &self.layout, &self.texture, &self.uniform);
            self.extent = extent;
            self.resident = None;
        }
        if self.resident != Some(paint.key) {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(&paint.field.values),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(paint.field.width * size_of::<f32>() as u32),
                    rows_per_image: Some(paint.field.height),
                },
                wgpu::Extent3d {
                    width: paint.field.width,
                    height: paint.field.height,
                    depth_or_array_layers: 1,
                },
            );
            self.resident = Some(paint.key);
        }
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&Uniform::forge(paint)));
    }
}

fn field_texture(device: &wgpu::Device, extent: [u32; 2]) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("hrrr-field"),
        size: wgpu::Extent3d {
            width: extent[0],
            height: extent[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[wgpu::TextureFormat::R32Float],
    })
}

fn field_bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    texture: &wgpu::Texture,
    uniform: &wgpu::Buffer,
) -> wgpu::BindGroup {
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("hrrr-field"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: uniform.as_entire_binding(),
            },
        ],
    })
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniform {
    world: [f32; 4],
    grid: [u32; 2],
    bin_count: u32,
    _pad_a: u32,
    affine: [f32; 2],
    contour_width: f32,
    _pad_b: f32,
    contour: [f32; 4],
    lambert_a: [f32; 4],
    lambert_b: [f32; 4],
    ceilings: [[f32; 4]; SCALE_CAPACITY],
    colors: [[f32; 4]; SCALE_CAPACITY],
}

impl Uniform {
    fn forge(paint: &FieldPaint) -> Self {
        let mut ceilings = [[0.0; 4]; SCALE_CAPACITY];
        let mut colors = [[0.0; 4]; SCALE_CAPACITY];
        for (slot, bin) in paint.scale.bins.iter().take(SCALE_CAPACITY).enumerate() {
            ceilings[slot][0] = bin.ceiling;
            colors[slot] = [
                srgb_to_linear(bin.srgb[0]),
                srgb_to_linear(bin.srgb[1]),
                srgb_to_linear(bin.srgb[2]),
                f32::from(bin.srgb[3]) / 255.0,
            ];
        }
        let LambertGrid {
            cone,
            radius_factor,
            origin_rho,
            central_lon,
            first_xy,
            spacing,
        } = paint.field.projection;
        Self {
            world: paint.world_bounds,
            grid: [paint.field.width, paint.field.height],
            bin_count: u32::try_from(paint.scale.bins.len().min(SCALE_CAPACITY)).unwrap_or(0),
            _pad_a: 0,
            affine: paint.scale.unit.affine(),
            contour_width: paint.scale.contour.width_points,
            _pad_b: 0.0,
            contour: [
                srgb_to_linear(paint.scale.contour.srgb[0]),
                srgb_to_linear(paint.scale.contour.srgb[1]),
                srgb_to_linear(paint.scale.contour.srgb[2]),
                f32::from(paint.scale.contour.srgb[3]) / 255.0,
            ],
            lambert_a: [
                cone as f32,
                radius_factor as f32,
                origin_rho as f32,
                central_lon as f32,
            ],
            lambert_b: [
                first_xy[0] as f32,
                first_xy[1] as f32,
                spacing[0] as f32,
                spacing[1] as f32,
            ],
            ceilings,
            colors,
        }
    }
}

fn srgb_to_linear(channel: u8) -> f32 {
    let encoded = f32::from(channel) / 255.0;
    if encoded <= 0.040_45 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

pub fn world_pixels(view: Viewport) -> f64 {
    TILE_EDGE * view.zoom.exp2()
}

fn ground_meters_per_point(view: Viewport) -> f64 {
    let latitude = lon_lat_at(view.center_mercator)[1].to_radians();
    EARTH_CIRCUMFERENCE_M * latitude.cos() / world_pixels(view)
}

pub fn minimum_zoom(viewport_height: f32) -> f64 {
    (f64::from(viewport_height.max(1.0)) / (TILE_EDGE * MAX_VERTICAL_WORLD_SPAN))
        .log2()
        .clamp(Viewport::MIN_ZOOM, Viewport::MAX_ZOOM)
}

pub fn world_bounds(view: Viewport, rect: egui::Rect) -> [f64; 4] {
    let scale = world_pixels(view);
    let half = rect.size() * 0.5;
    [
        view.center_mercator[0] - f64::from(half.x) / scale,
        view.center_mercator[1] - f64::from(half.y) / scale,
        view.center_mercator[0] + f64::from(half.x) / scale,
        view.center_mercator[1] + f64::from(half.y) / scale,
    ]
}

pub fn world_at(view: Viewport, rect: egui::Rect, point: egui::Pos2) -> [f64; 2] {
    let scale = world_pixels(view);
    [
        view.center_mercator[0] + f64::from(point.x - rect.center().x) / scale,
        view.center_mercator[1] + f64::from(point.y - rect.center().y) / scale,
    ]
}

pub fn screen_at(view: Viewport, rect: egui::Rect, world: [f64; 2]) -> egui::Pos2 {
    let scale = world_pixels(view);
    rect.center()
        + egui::vec2(
            ((world[0] - view.center_mercator[0]) * scale) as f32,
            ((world[1] - view.center_mercator[1]) * scale) as f32,
        )
}

pub fn lon_lat_at(world: [f64; 2]) -> [f64; 2] {
    let longitude = world[0].mul_add(360.0, -180.0);
    let latitude = (std::f64::consts::PI * (1.0 - 2.0 * world[1]))
        .sinh()
        .atan()
        .to_degrees();
    [longitude, latitude]
}

pub fn grid_at(field: &FieldGrid, world: [f64; 2]) -> [f64; 2] {
    let [longitude, latitude] = lon_lat_at(world);
    field.projection.grid_at_lon_lat(longitude, latitude)
}

pub fn visible_peak(field: &FieldGrid, view: Viewport, rect: egui::Rect) -> Option<f32> {
    let columns = (rect.width() / VISIBLE_SAMPLE_PITCH).ceil().max(1.0) as u32;
    let rows = (rect.height() / VISIBLE_SAMPLE_PITCH).ceil().max(1.0) as u32;
    let mut peak: Option<f32> = None;
    for row in 0..rows {
        let y = egui::lerp(rect.y_range(), (row as f32 + 0.5) / rows as f32);
        for column in 0..columns {
            let x = egui::lerp(rect.x_range(), (column as f32 + 0.5) / columns as f32);
            let [i, j] = grid_at(field, world_at(view, rect, egui::pos2(x, y))).map(f64::round);
            if !(0.0..f64::from(field.width)).contains(&i)
                || !(0.0..f64::from(field.height)).contains(&j)
            {
                continue;
            }
            if let Some(value) = field
                .at(i as u32, j as u32)
                .filter(|value| value.is_finite())
            {
                peak = Some(peak.map_or(value, |peak| peak.max(value)));
            }
        }
    }
    peak
}

fn pleasant_length(target: f64) -> f64 {
    let exponent = target.max(1.0).log10().floor();
    let magnitude = 10_f64.powf(exponent);
    let unit = target / magnitude;
    let step = if unit >= 5.0 {
        5.0
    } else if unit >= 2.0 {
        2.0
    } else {
        1.0
    };
    step * magnitude
}

fn paint_scale_length(
    painter: &egui::Painter,
    rect: egui::Rect,
    meters: f64,
    meters_per_point: f64,
    maturity: f32,
) {
    let width = (meters / meters_per_point) as f32;
    let origin = egui::pos2(rect.left() + 18.0, rect.bottom() - 19.0);
    let paper = egui::Color32::from_white_alpha(220).gamma_multiply(maturity);
    let ink = egui::Color32::from_rgb(35, 31, 26).gamma_multiply(maturity);
    for stroke in [
        egui::Stroke::new(4.0_f32, paper),
        egui::Stroke::new(1.5_f32, ink),
    ] {
        let _bar = painter.line_segment([origin, origin + egui::vec2(width, 0.0)], stroke);
        let _left = painter.line_segment(
            [origin - egui::vec2(0.0, 4.0), origin + egui::vec2(0.0, 4.0)],
            stroke,
        );
        let _right = painter.line_segment(
            [
                origin + egui::vec2(width, -4.0),
                origin + egui::vec2(width, 4.0),
            ],
            stroke,
        );
    }
    let label = if meters >= 1_000.0 {
        format!("{:.0} km", meters / 1_000.0)
    } else {
        format!("{meters:.0} m")
    };
    let anchor = origin + egui::vec2(width * 0.5, -5.0);
    let font = egui::FontId::monospace(11.0);
    for offset in [
        egui::vec2(-1.0, 0.0),
        egui::vec2(1.0, 0.0),
        egui::vec2(0.0, -1.0),
        egui::vec2(0.0, 1.0),
    ] {
        let _halo = painter.text(
            anchor + offset,
            egui::Align2::CENTER_BOTTOM,
            &label,
            font.clone(),
            paper,
        );
    }
    let _ink = painter.text(anchor, egui::Align2::CENTER_BOTTOM, label, font, ink);
}

const WGSL: &str = r"
struct Uniform {
    world: vec4f,
    grid: vec2u,
    bin_count: u32,
    pad_a: u32,
    affine: vec2f,
    contour_width: f32,
    pad_b: f32,
    contour: vec4f,
    lambert_a: vec4f,
    lambert_b: vec4f,
    ceilings: array<vec4f, 128>,
    colors: array<vec4f, 128>,
};

@group(0) @binding(0) var field: texture_2d<f32>;
@group(0) @binding(1) var<uniform> u: Uniform;

struct VertexOut {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
};

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> VertexOut {
    let positions = array(vec2f(-1.0, -1.0), vec2f(3.0, -1.0), vec2f(-1.0, 3.0));
    let uvs = array(vec2f(0.0, 1.0), vec2f(2.0, 1.0), vec2f(0.0, -1.0));
    var out: VertexOut;
    out.position = vec4f(positions[index], 0.0, 1.0);
    out.uv = uvs[index];
    return out;
}

fn grid_at(world: vec2f) -> vec2f {
    let longitude = world.x * 6.28318530718 - 3.14159265359;
    let latitude = atan(sinh(3.14159265359 * (1.0 - 2.0 * world.y)));
    let cone = u.lambert_a.x;
    let rho = u.lambert_a.y / pow(tan(0.78539816339 + latitude * 0.5), cone);
    let delta = longitude - u.lambert_a.w;
    let theta = cone * atan2(sin(delta), cos(delta));
    let xy = vec2f(rho * sin(theta), u.lambert_a.z - rho * cos(theta));
    return (xy - u.lambert_b.xy) / u.lambert_b.zw;
}

fn raw_at(cell: vec2f) -> f32 {
    let lo = vec2i(floor(cell));
    let hi = lo + vec2i(1);
    let edge = vec2i(u.grid) - vec2i(1);
    let a = textureLoad(field, clamp(lo, vec2i(0), edge), 0).x;
    let b = textureLoad(field, clamp(vec2i(hi.x, lo.y), vec2i(0), edge), 0).x;
    let c = textureLoad(field, clamp(vec2i(lo.x, hi.y), vec2i(0), edge), 0).x;
    let d = textureLoad(field, clamp(hi, vec2i(0), edge), 0).x;
    let f = fract(cell);
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

fn bin_slot(value: f32) -> u32 {
    var lo = 0u;
    var hi = u.bin_count;
    while lo < hi {
        let mid = (lo + hi) / 2u;
        if value <= u.ceilings[mid].x {
            hi = mid;
        } else {
            lo = mid + 1u;
        }
    }
    return min(lo, u.bin_count - 1u);
}

fn bin_color(value: f32) -> vec4f {
    if u.bin_count == 0u { return vec4f(0.0); }
    return u.colors[bin_slot(value)];
}

fn incise_contours(value: f32, base: vec4f) -> vec4f {
    if u.bin_count <= 1u || u.contour_width <= 0.0 { return base; }
    let slope = fwidth(value);
    if slope <= 0.000001 { return base; }
    let slot = bin_slot(value);
    var distance = 1e20;
    if slot + 1u < u.bin_count {
        distance = abs(value - u.ceilings[slot].x) / slope;
    }
    if slot > 0u {
        distance = min(distance, abs(value - u.ceilings[slot - 1u].x) / slope);
    }
    let coverage = 1.0 - smoothstep(u.contour_width, u.contour_width + 0.85, distance);
    let ink = coverage * u.contour.a;
    return vec4f(mix(base.rgb, u.contour.rgb, ink), max(base.a, ink));
}

@fragment
fn fragment(in: VertexOut) -> @location(0) vec4f {
    let world = mix(u.world.xy, u.world.zw, in.uv);
    let cell = grid_at(world);
    let edge = vec2f(u.grid - vec2u(1u));
    if any(cell < vec2f(0.0)) || any(cell > edge) { return vec4f(0.0); }
    let raw = raw_at(cell);
    if raw != raw { return vec4f(0.0); }
    let value = raw * u.affine.x + u.affine.y;
    return incise_contours(value, bin_color(value));
}
";

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{Context as _, Result};
    use std::{hint::black_box, time::Instant};

    #[test]
    fn default_view_is_centered_on_the_conus() {
        let [longitude, latitude] = lon_lat_at(Viewport::default().center_mercator);
        assert!((longitude + 97.5).abs() < 0.01);
        assert!((latitude - 38.5).abs() < 0.01);
    }

    #[test]
    fn zoom_floor_frames_north_america() {
        let height = 920.0;
        let view = Viewport {
            zoom: minimum_zoom(height),
            ..Viewport::default()
        };
        let half_span = f64::from(height) / world_pixels(view) * 0.5;
        let north = lon_lat_at([0.5, view.center_mercator[1] - half_span])[1];
        let south = lon_lat_at([0.5, view.center_mercator[1] + half_span])[1];
        assert!((half_span.mul_add(2.0, -MAX_VERTICAL_WORLD_SPAN)).abs() < 1e-12);
        assert!((58.0..62.0).contains(&north));
        assert!((5.0..15.0).contains(&south));
    }

    #[test]
    fn zoom_ceiling_spans_about_two_kilometres_at_the_default_latitude() {
        let view = Viewport {
            zoom: Viewport::MAX_ZOOM,
            ..Viewport::default()
        };
        let full_hd_span_m = ground_meters_per_point(view) * 1_920.0;
        assert!((1_950.0..=2_050.0).contains(&full_hd_span_m));
    }

    #[test]
    fn scale_lengths_are_one_two_or_five_decades() {
        for target in [3.0, 27.0, 650.0, 8_400.0] {
            let length = pleasant_length(target);
            let decade = 10_f64.powf(length.log10().floor());
            assert!([1.0, 2.0, 5.0].contains(&(length / decade)));
            assert!(length <= target);
        }
    }

    #[test]
    #[ignore = "release-mode microprofile"]
    fn profile_visible_peak() -> Result<()> {
        let projection = LambertGrid::forge(
            6_371_229.0,
            21.138,
            237.28,
            38.5,
            262.5,
            [38.5, 38.5],
            [3_000.0, 3_000.0],
        )?;
        let field = FieldGrid::forge(vec![1.0e-9; 1_799 * 1_059], 1_799, 1_059, projection)?;
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1_146.0, 920.0));
        let begun = Instant::now();
        let iterations = 100;
        for _ in 0..iterations {
            let peak = visible_peak(black_box(&field), Viewport::default(), rect)
                .context("visible HRRR sample")?;
            let _peak = black_box(peak);
        }
        eprintln!(
            "visible-peak samples={} mean_us={}",
            iterations,
            begun.elapsed().as_micros() / iterations
        );
        Ok(())
    }
}
