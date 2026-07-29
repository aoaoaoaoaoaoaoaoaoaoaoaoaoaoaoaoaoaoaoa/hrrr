use crate::basemap::{FillPoint, StrokePoint, TileKey, VectorTile};
use bytemuck::{Pod, Zeroable};
use egui_wgpu::{CallbackResources, CallbackTrait, ScreenDescriptor, wgpu};
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    sync::Arc,
    time::Instant,
};
use wgpu::util::DeviceExt as _;

const GPU_CEILING: usize = 384 * 1_048_576;
const MAX_WRAP_RADIUS: u32 = 2;
const MAX_WRAP_INSTANCES: usize = (MAX_WRAP_RADIUS * 2 + 1) as usize;
const PLATE_TILE_EDGE: f64 = 256.0;
const PLATE_PERIOD: f64 = 192.0;

#[derive(Clone)]
pub struct VectorPaint {
    pub tiles: Arc<[Arc<VectorTile>]>,
    pub center_world: [f64; 2],
    pub world_points: f32,
    pub viewport_points: [f32; 2],
    pub view_zoom: f32,
    pub apparition_span: f32,
}

impl CallbackTrait for VectorPaint {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(gpu) = resources.get_mut::<VectorMapGpu>() {
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
        let Some(gpu) = resources.get::<VectorMapGpu>() else {
            return;
        };
        pass.set_bind_group(0, &gpu.bind, &[]);
        pass.set_pipeline(&gpu.fill_pipeline);
        for key in &gpu.active {
            if let Some(tile) = gpu.tiles.get(key)
                && let Some(draw) = &tile.fills
            {
                draw.paint(pass, &tile.buffer, &tile.transform, gpu.instances);
            }
        }
        pass.set_pipeline(&gpu.stroke_pipeline);
        for key in &gpu.active {
            if let Some(tile) = gpu.tiles.get(key)
                && let Some(draw) = &tile.strokes
            {
                draw.paint(pass, &tile.buffer, &tile.transform, gpu.instances);
            }
        }
    }
}

pub struct VectorMapGpu {
    fill_pipeline: wgpu::RenderPipeline,
    stroke_pipeline: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    bind: wgpu::BindGroup,
    tiles: HashMap<TileKey, GpuTile>,
    active: Vec<TileKey>,
    active_set: HashSet<TileKey>,
    epoch: u64,
    bytes: usize,
    instances: u32,
    profile: bool,
}

impl VectorMapGpu {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vector-map-uniform"),
            size: size_of::<Uniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vector-map"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(size_of::<Uniform>() as u64),
                },
                count: None,
            }],
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vector-map"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vector-map"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vector-map"),
            source: wgpu::ShaderSource::Wgsl(WGSL.into()),
        });
        let fill_pipeline = pipeline(
            device,
            format,
            &pipeline_layout,
            &shader,
            "vector-fill",
            "fill_vertex",
            fragment_entry(format),
            fill_layout(),
        );
        let stroke_pipeline = pipeline(
            device,
            format,
            &pipeline_layout,
            &shader,
            "vector-stroke",
            "stroke_vertex",
            fragment_entry(format),
            stroke_layout(),
        );
        Self {
            fill_pipeline,
            stroke_pipeline,
            uniform,
            bind,
            tiles: HashMap::new(),
            active: Vec::new(),
            active_set: HashSet::new(),
            epoch: 0,
            bytes: 0,
            instances: 1,
            profile: std::env::var_os("HRRR_PROFILE_BASEMAP").is_some(),
        }
    }

    fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, paint: &VectorPaint) {
        let begun = Instant::now();
        let incoming = paint.tiles.iter().map(|tile| tile.key).collect::<Vec<_>>();
        let changed = incoming != self.active;
        if changed {
            self.epoch = self.epoch.saturating_add(1);
            incoming.clone_into(&mut self.active);
            self.active_set.clear();
            self.active_set.extend(incoming.iter().copied());
            for key in &incoming {
                if let Some(resident) = self.tiles.get_mut(key) {
                    resident.touched = self.epoch;
                }
            }
        }
        let mut uploaded = 0_usize;
        for tile in paint.tiles.iter() {
            if self.tiles.contains_key(&tile.key) {
                continue;
            }
            let resident = GpuTile::raise(device, tile, self.epoch);
            uploaded = uploaded.saturating_add(resident.bytes);
            self.bytes = self.bytes.saturating_add(resident.bytes);
            let _prior = self.tiles.insert(tile.key, resident);
        }
        let uniform = Uniform::forge(paint);
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniform));
        self.instances = uniform.wrap_radius.saturating_mul(2).saturating_add(1);
        self.reap();
        if self.profile {
            eprintln!(
                "vector-gpu prepare_us={} upload_bytes={uploaded} active_tiles={} changed={changed}",
                begun.elapsed().as_micros(),
                self.active.len()
            );
        }
    }

    fn reap(&mut self) {
        if self.bytes <= GPU_CEILING {
            return;
        }
        while self.bytes > GPU_CEILING {
            let victim = self
                .tiles
                .iter()
                .filter(|(key, _resident)| !self.active_set.contains(key))
                .min_by_key(|(_key, resident)| resident.touched)
                .map(|(key, _resident)| *key);
            let Some(key) = victim else { break };
            let Some(victim) = self.tiles.remove(&key) else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(victim.bytes);
        }
    }
}

fn pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    label: &'static str,
    vertex_entry: &'static str,
    fragment_entry: &'static str,
    vertex: wgpu::VertexBufferLayout<'static>,
) -> wgpu::RenderPipeline {
    let buffers = [vertex, tile_layout()];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(vertex_entry),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &buffers,
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment_entry),
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
    })
}

fn fragment_entry(format: wgpu::TextureFormat) -> &'static str {
    if format.is_srgb() {
        "fragment_linear"
    } else {
        "fragment_gamma"
    }
}

fn fill_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRIBUTES: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![
        0 => Float32x2,
        1 => Uint32,
        7 => Float32
    ];
    wgpu::VertexBufferLayout {
        array_stride: size_of::<FillPoint>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &ATTRIBUTES,
    }
}

fn stroke_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
        0 => Float32x2,
        1 => Float32x2,
        2 => Unorm8x4,
        3 => Float32,
        7 => Float32
    ];
    wgpu::VertexBufferLayout {
        array_stride: size_of::<StrokePoint>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &ATTRIBUTES,
    }
}

fn tile_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRIBUTES: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![
        4 => Float32x2,
        5 => Float32x2,
        6 => Float32
    ];
    wgpu::VertexBufferLayout {
        array_stride: size_of::<TileInstance>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &ATTRIBUTES,
    }
}

struct GpuTile {
    fills: Option<Draw>,
    strokes: Option<Draw>,
    buffer: wgpu::Buffer,
    transform: Range<u64>,
    bytes: usize,
    touched: u64,
}

impl GpuTile {
    fn raise(device: &wgpu::Device, tile: &VectorTile, touched: u64) -> Self {
        let mut blade = Vec::with_capacity(
            tile.resident_bytes()
                .saturating_add(size_of::<TileInstance>() * MAX_WRAP_INSTANCES),
        );
        let fills = Draw::pack(&mut blade, &tile.fills.vertices, &tile.fills.indices);
        let strokes = Draw::pack(&mut blade, &tile.strokes.vertices, &tile.strokes.indices);
        let transform = append(
            &mut blade,
            &[TileInstance::forge(tile.key); MAX_WRAP_INSTANCES],
        );
        let bytes = blade.len();
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vector-tile"),
            contents: &blade,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::INDEX,
        });
        Self {
            fills,
            strokes,
            buffer,
            transform,
            bytes,
            touched,
        }
    }
}

struct Draw {
    vertices: Range<u64>,
    indices: Range<u64>,
    index_count: u32,
}

impl Draw {
    fn pack<V: Pod>(blade: &mut Vec<u8>, vertices: &[V], indices: &[u32]) -> Option<Self> {
        if vertices.is_empty() || indices.is_empty() {
            return None;
        }
        let index_count = u32::try_from(indices.len()).ok()?;
        let vertices = append(blade, vertices);
        let indices = append(blade, indices);
        Some(Self {
            vertices,
            indices,
            index_count,
        })
    }

    fn paint(
        &self,
        pass: &mut wgpu::RenderPass<'static>,
        buffer: &wgpu::Buffer,
        transform: &Range<u64>,
        instances: u32,
    ) {
        pass.set_vertex_buffer(0, buffer.slice(self.vertices.clone()));
        pass.set_vertex_buffer(1, buffer.slice(transform.clone()));
        pass.set_index_buffer(
            buffer.slice(self.indices.clone()),
            wgpu::IndexFormat::Uint32,
        );
        pass.draw_indexed(0..self.index_count, 0, 0..instances);
    }
}

fn append<T: Pod>(blade: &mut Vec<u8>, values: &[T]) -> Range<u64> {
    let start = blade.len() as u64;
    blade.extend_from_slice(bytemuck::cast_slice(values));
    start..blade.len() as u64
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniform {
    center_high: [f32; 2],
    center_low: [f32; 2],
    viewport: [f32; 2],
    plate_phase: [f32; 2],
    world_points: f32,
    wrap_radius: u32,
    view_zoom: f32,
    apparition_span: f32,
}

impl Uniform {
    fn forge(paint: &VectorPaint) -> Self {
        let [x_high, x_low] = split(paint.center_world[0]);
        let [y_high, y_low] = split(paint.center_world[1]);
        let plate_points = plate_points(paint.view_zoom);
        let wrap_radius = wrap_radius(
            paint.viewport_points[0] / paint.world_points,
            paint.center_world[0] as f32,
        );
        Self {
            center_high: [x_high, y_high],
            center_low: [x_low, y_low],
            viewport: paint.viewport_points,
            plate_phase: paint
                .center_world
                .map(|axis| plate_phase(axis, plate_points)),
            world_points: paint.world_points,
            wrap_radius,
            view_zoom: paint.view_zoom,
            apparition_span: paint.apparition_span,
        }
    }
}

fn plate_points(view_zoom: f32) -> f32 {
    (PLATE_TILE_EDGE * f64::from(view_zoom.floor()).exp2()) as f32
}

fn plate_phase(world: f64, world_points: f32) -> f32 {
    (world * f64::from(world_points)).rem_euclid(PLATE_PERIOD) as f32
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TileInstance {
    origin_high: [f32; 2],
    origin_low: [f32; 2],
    span: f32,
    _pad: [f32; 3],
}

impl TileInstance {
    fn forge(key: TileKey) -> Self {
        let divisions = f64::from(1_u32 << key.zoom);
        let [x_high, x_low] = split(f64::from(key.x) / divisions);
        let [y_high, y_low] = split(f64::from(key.y) / divisions);
        Self {
            origin_high: [x_high, y_high],
            origin_low: [x_low, y_low],
            span: (1.0 / divisions) as f32,
            _pad: [0.0; 3],
        }
    }
}

fn split(value: f64) -> [f32; 2] {
    let high = value as f32;
    [high, (value - f64::from(high)) as f32]
}

fn wrap_radius(world_width: f32, center_x: f32) -> u32 {
    let half = world_width * 0.5;
    let west = (half - center_x).max(0.0);
    let east = (center_x + half - 1.0).max(0.0);
    (west.max(east).ceil() as u32).min(MAX_WRAP_RADIUS)
}

const WGSL: &str = r"
struct Uniform {
    center_high: vec2f,
    center_low: vec2f,
    viewport: vec2f,
    plate_phase: vec2f,
    world_points: f32,
    wrap_radius: u32,
    view_zoom: f32,
    apparition_span: f32,
};

@group(0) @binding(0) var<uniform> u: Uniform;

struct VertexOut {
    @builtin(position) position: vec4f,
    @location(0) color: vec4f,
    @location(1) edge_distance: f32,
    @location(2) solid_radius: f32,
    @location(3) tile_local: vec2f,
    @location(4) plate_point: vec2f,
    @location(5) @interpolate(flat) material: u32,
};

fn apparition(onset_zoom: f32) -> f32 {
    let phase = clamp(
        (u.view_zoom - onset_zoom) / max(u.apparition_span, 0.001),
        0.0,
        1.0,
    );
    return phase * phase * (3.0 - 2.0 * phase);
}

fn points_at(
    local: vec2f,
    origin_high: vec2f,
    origin_low: vec2f,
    tile_span: f32,
    instance: u32,
) -> vec2f {
    let origin_delta = (origin_high - u.center_high)
        + (origin_low - u.center_low);
    var delta = origin_delta + local * tile_span;
    // A tile is an indivisible chart. Per-vertex wrapping tears coarse
    // triangles across the antimeridian into screen-spanning shards.
    delta.x -= round(origin_delta.x + tile_span * 0.5);
    delta.x += f32(instance) - f32(u.wrap_radius);
    return delta * u.world_points;
}

fn clip_at(points: vec2f) -> vec2f {
    return vec2f(points.x * 2.0 / u.viewport.x, -points.y * 2.0 / u.viewport.y);
}

@vertex
fn fill_vertex(
    @location(0) local: vec2f,
    @location(1) material: u32,
    @location(7) onset_zoom: f32,
    @location(4) origin_high: vec2f,
    @location(5) origin_low: vec2f,
    @location(6) tile_span: f32,
    @builtin(instance_index) instance: u32,
) -> VertexOut {
    var out: VertexOut;
    let points = points_at(local, origin_high, origin_low, tile_span, instance);
    out.position = vec4f(
        clip_at(points),
        0.0,
        1.0,
    );
    let maturity = apparition(onset_zoom);
    out.color = vec4f(1.0, 1.0, 1.0, maturity);
    out.edge_distance = 0.0;
    out.solid_radius = -1.0;
    out.tile_local = local;
    out.plate_point = points * exp2(floor(u.view_zoom) - u.view_zoom) + u.plate_phase;
    out.material = material;
    return out;
}

@vertex
fn stroke_vertex(
    @location(0) local: vec2f,
    @location(1) extrusion: vec2f,
    @location(2) color: vec4f,
    @location(3) radius: f32,
    @location(7) onset_side: f32,
    @location(4) origin_high: vec2f,
    @location(5) origin_low: vec2f,
    @location(6) tile_span: f32,
    @builtin(instance_index) instance: u32,
) -> VertexOut {
    var out: VertexOut;
    let onset_zoom = abs(onset_side) - 1.0;
    let side = sign(onset_side);
    let maturity = apparition(onset_zoom);
    let visible_radius = radius * mix(0.12, 1.0, maturity);
    let expanded_radius = visible_radius + 0.8;
    let offset = extrusion * expanded_radius * 2.0 / u.viewport;
    let points = points_at(local, origin_high, origin_low, tile_span, instance);
    out.position = vec4f(
        clip_at(points) + vec2f(offset.x, -offset.y),
        0.0,
        1.0,
    );
    out.color = vec4f(color.rgb, color.a * mix(0.16, 1.0, maturity));
    out.edge_distance = side * expanded_radius;
    out.solid_radius = visible_radius;
    out.tile_local = local
        + extrusion * expanded_radius / (u.world_points * tile_span);
    out.plate_point = points * exp2(floor(u.view_zoom) - u.view_zoom) + u.plate_phase;
    out.material = 0xffffffffu;
    return out;
}

fn accretion() -> f32 {
    return smoothstep(0.08, 0.92, fract(u.view_zoom));
}

fn ruled_at(value: f32, period: f32, offset: f32, half_width: f32) -> f32 {
    let gradient = max(fwidth(value), 0.0001);
    let folded = abs(fract((value - offset) / period + 0.5) - 0.5) * period;
    let radius = half_width * gradient;
    let antialias = gradient * 0.62;
    return 1.0 - smoothstep(
        max(radius - antialias, 0.0),
        radius + antialias,
        folded,
    );
}

fn nested_rule(value: f32, period: f32, half_width: f32) -> f32 {
    let elder = ruled_at(value, period, 0.0, half_width);
    let newborn = ruled_at(value, period, period * 0.5, half_width);
    return max(elder, newborn * accretion());
}

fn dotted_at(point: vec2f, period: f32, offset: vec2f, radius: f32) -> f32 {
    let delta = (fract((point - offset) / period + 0.5) - 0.5) * period;
    let distance = length(delta);
    let gradient = max(max(fwidth(point.x), fwidth(point.y)), 0.0001);
    let dot_radius = radius * gradient;
    let antialias = gradient * 0.62;
    return 1.0 - smoothstep(
        max(dot_radius - antialias, 0.0),
        dot_radius + antialias,
        distance,
    );
}

fn nested_dot(point: vec2f, period: f32, radius: f32) -> f32 {
    let half = period * 0.5;
    let elder = dotted_at(point, period, vec2f(0.0), radius);
    let edge = max(
        dotted_at(point, period, vec2f(half, 0.0), radius),
        dotted_at(point, period, vec2f(0.0, half), radius),
    );
    let heart = dotted_at(point, period, vec2f(half), radius);
    let phase = fract(u.view_zoom);
    let edge_birth = smoothstep(0.04, 0.78, phase);
    let heart_birth = smoothstep(0.22, 0.94, phase);
    return max(elder, max(edge * edge_birth, heart * heart_birth));
}

fn ink(mark: f32, mean: f32, value: f32, opacity: f32, maturity: f32) -> vec4f {
    let disclosure = smoothstep(6.25, 9.25, u.view_zoom);
    let coverage = mean + mark * disclosure * (1.0 - mean);
    return vec4f(vec3f(value), coverage * opacity * maturity);
}

fn plate(material: u32, point: vec2f, maturity: f32) -> vec4f {
    if material == 0u {
        return vec4f(vec3f(0.825), maturity);
    }
    if material == 1u {
        let mark = nested_dot(point, 8.0, 0.72);
        return ink(mark, 0.045, 0.27, 0.74, maturity);
    }
    if material == 2u {
        let mark = nested_rule(point.x + point.y, 12.0, 0.24);
        return ink(mark, 0.038, 0.38, 0.54, maturity);
    }
    if material == 3u {
        let mark = nested_rule(point.y, 8.0, 0.22);
        return ink(mark, 0.032, 0.31, 0.62, maturity);
    }
    if material == 4u {
        let mark = max(
            nested_rule(point.x + point.y, 16.0, 0.22),
            nested_rule(point.x - point.y, 16.0, 0.22),
        );
        return ink(mark, 0.043, 0.40, 0.45, maturity);
    }
    if material == 5u {
        let mark = nested_dot(point, 12.0, 0.50);
        return ink(mark, 0.014, 0.42, 0.44, maturity);
    }
    if material == 6u {
        let mark = max(
            nested_rule(point.x + point.y, 8.0, 0.22),
            nested_rule(point.x - point.y, 8.0, 0.22),
        );
        return ink(mark, 0.078, 0.22, 0.58, maturity);
    }
    if material == 7u {
        let mark = nested_dot(point, 8.0, 0.58);
        return ink(mark, 0.025, 0.29, 0.48, maturity);
    }
    if material == 8u {
        let mark = nested_dot(point, 6.0, 0.52);
        return ink(mark, 0.040, 0.31, 0.38, maturity);
    }
    if material == 9u {
        let waterline = nested_rule(point.y, 16.0, 0.18);
        let value = mix(0.915, 0.54, waterline * 0.24);
        return vec4f(vec3f(value), maturity);
    }
    return vec4f(0.0);
}

fn painted(in: VertexOut) -> vec4f {
    // MVTs overlap their neighbors; half-open ownership prevents translucent
    // skirts from double-blending into visible tile seams.
    if any(in.tile_local < vec2f(0.0)) || any(in.tile_local >= vec2f(1.0)) {
        discard;
    }
    if in.solid_radius < 0.0 {
        return plate(in.material, in.plate_point, in.color.a);
    }
    let feather = max(fwidth(in.edge_distance), 0.65);
    let coverage = clamp(
        (in.solid_radius + feather * 0.5 - abs(in.edge_distance)) / feather,
        0.0,
        1.0,
    );
    return vec4f(in.color.rgb, in.color.a * coverage);
}

@fragment
fn fragment_gamma(in: VertexOut) -> @location(0) vec4f {
    return painted(in);
}

fn linear_channel(encoded: f32) -> f32 {
    if encoded <= 0.04045 { return encoded / 12.92; }
    return pow((encoded + 0.055) / 1.055, 2.4);
}

@fragment
fn fragment_linear(in: VertexOut) -> @location(0) vec4f {
    let color = painted(in);
    return vec4f(
        linear_channel(color.r),
        linear_channel(color.g),
        linear_channel(color.b),
        color.a,
    );
}
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framebuffer_transfer_function_matches_egui() {
        assert_eq!(
            fragment_entry(wgpu::TextureFormat::Bgra8Unorm),
            "fragment_gamma"
        );
        assert_eq!(
            fragment_entry(wgpu::TextureFormat::Bgra8UnormSrgb),
            "fragment_linear"
        );
    }

    #[test]
    fn stroke_vertex_keeps_onset_and_side_inside_seven_words() {
        assert_eq!(size_of::<StrokePoint>(), 7 * size_of::<f32>());
    }

    #[test]
    fn fill_vertex_is_four_words() {
        assert_eq!(size_of::<FillPoint>(), 4 * size_of::<f32>());
    }

    #[test]
    fn plate_phase_is_exactly_periodic() {
        let scale = 256.0_f32 * 16_777_216.0;
        let a = plate_phase(0.294_117_332_617_894_8, scale);
        let b = plate_phase(
            0.294_117_332_617_894_8 + PLATE_PERIOD / f64::from(scale),
            scale,
        );
        assert!((a - b).abs() < 0.000_1);
    }

    #[test]
    fn plate_lattice_refines_dyadically() {
        assert_eq!(plate_points(7.0), plate_points(7.999));
        assert_eq!(plate_points(8.0), plate_points(7.0) * 2.0);

        let world = 0.294_117_332_617_894_8;
        let elder = plate_phase(world, plate_points(11.0));
        let heir = plate_phase(world, plate_points(12.0));
        let expected = (f64::from(elder) * 2.0).rem_euclid(PLATE_PERIOD) as f32;
        assert!((heir - expected).abs() < 0.000_1, "{heir} != {expected}");
    }

    #[test]
    fn split_coordinates_hold_subpixel_precision_at_z24() {
        let center = 0.229_166_666_666_666_67_f64;
        let zoom = 24.0_f64;
        let world_points = 256.0 * zoom.exp2();
        let key = TileKey {
            zoom: 12,
            x: (center * 4096.0).floor() as u32,
            y: 0,
        };
        let tile = TileInstance::forge(key);
        let local = 1234.0_f32 / 4096.0;
        let [center_high, center_low] = split(center);
        let actual = ((tile.origin_high[0] - center_high)
            + (tile.origin_low[0] - center_low)
            + local * tile.span) as f64
            * world_points;
        let point = (f64::from(key.x) + f64::from(local)) / 4096.0;
        let expected = (point - center) * world_points;
        assert!((actual - expected).abs() < 0.1, "{actual} != {expected}");
    }

    #[test]
    fn repetition_covers_every_world_crossing() {
        assert_eq!(wrap_radius(0.99, 0.5), 0);
        assert_eq!(wrap_radius(0.99, 0.229), 1);
        assert_eq!(wrap_radius(0.1, 0.99), 1);
        assert_eq!(wrap_radius(1.01, 0.5), 1);
        assert_eq!(wrap_radius(2.2, 0.5), 1);
        assert_eq!(wrap_radius(3.2, 0.5), 2);
        assert_eq!(
            wrap_radius(f32::INFINITY, 0.5) * 2 + 1,
            MAX_WRAP_INSTANCES as u32
        );
    }
}
