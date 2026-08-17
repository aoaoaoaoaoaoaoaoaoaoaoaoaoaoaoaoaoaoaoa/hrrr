//! Stable, world-anchored wind direction stamped through an app-owned die.

use crate::{
    map,
    model::{FieldGrid, Viewport},
};
use brass_poolrooms::chrome::{ForgedMesh, ForgedVertex};
use std::f32::consts::TAU;

mod baked {
    use super::{ForgedMesh, ForgedVertex};

    include!(concat!(env!("OUT_DIR"), "/wind_quill.rs"));
}

const TARGET_PITCH: f32 = 68.0;
const QUIET_WIND: f32 = 0.45;

pub fn paint(painter: &egui::Painter, field: &FieldGrid, view: Viewport, rect: egui::Rect) {
    let Some((columns, rows)) = visible_grid(field, view, rect) else {
        return;
    };
    let stride = stride(field, view, rect);
    let [column_lo, column_hi] = columns;
    let [row_lo, row_hi] = rows;
    let mut mesh = egui::Mesh::default();
    for j in lattice(row_lo, row_hi, stride) {
        for i in lattice(column_lo, column_hi, stride) {
            let Some([eastward, northward]) = field
                .vector_at(i, j)
                .filter(|wind| wind.iter().all(|component| component.is_finite()))
            else {
                continue;
            };
            if eastward.hypot(northward) < QUIET_WIND {
                continue;
            }
            let world = world_at_grid(field, i, j);
            let anchor = map::screen_at(view, rect, world);
            if !rect.shrink(8.0).contains(anchor) {
                continue;
            }
            let angle = eastward.atan2(northward).rem_euclid(TAU);
            let pose =
                ((angle / TAU * baked::POSE_COUNT as f32).round() as usize) % baked::POSE_COUNT;
            baked::QUILL[pose].stamp(&mut mesh, anchor);
        }
    }
    if !mesh.indices.is_empty() {
        let _quills = painter.add(egui::Shape::mesh(mesh));
    }
}

fn visible_grid(
    field: &FieldGrid,
    view: Viewport,
    rect: egui::Rect,
) -> Option<([u32; 2], [u32; 2])> {
    let cells = [
        rect.left_top(),
        rect.right_top(),
        rect.left_bottom(),
        rect.right_bottom(),
    ]
    .map(|point| map::grid_at(field, map::world_at(view, rect, point)));
    let bounds = |axis: usize, edge: f64| {
        let lo = cells
            .iter()
            .map(|cell| cell[axis])
            .fold(f64::INFINITY, f64::min)
            .floor()
            .clamp(0.0, edge);
        let hi = cells
            .iter()
            .map(|cell| cell[axis])
            .fold(f64::NEG_INFINITY, f64::max)
            .ceil()
            .clamp(0.0, edge);
        (lo <= hi).then_some([lo as u32, hi as u32])
    };
    Some((
        bounds(0, f64::from(field.width.saturating_sub(1)))?,
        bounds(1, f64::from(field.height.saturating_sub(1)))?,
    ))
}

fn stride(field: &FieldGrid, view: Viewport, rect: egui::Rect) -> u32 {
    let [i, j] = map::grid_at(field, view.center_mercator);
    let center = map::screen_at(view, rect, world_at_grid_fractional(field, i, j));
    let east = map::screen_at(view, rect, world_at_grid_fractional(field, i + 1.0, j));
    let north = map::screen_at(view, rect, world_at_grid_fractional(field, i, j + 1.0));
    let cell = ((center.distance(east) + center.distance(north)) * 0.5).max(0.01);
    let demand = (TARGET_PITCH / cell).max(1.0);
    2_u32.pow(demand.log2().ceil().clamp(0.0, 8.0) as u32)
}

fn lattice(lo: u32, hi: u32, stride: u32) -> impl Iterator<Item = u32> {
    let first = lo.div_ceil(stride) * stride;
    (first..=hi).step_by(stride as usize)
}

fn world_at_grid(field: &FieldGrid, i: u32, j: u32) -> [f64; 2] {
    world_at_grid_fractional(field, f64::from(i), f64::from(j))
}

fn world_at_grid_fractional(field: &FieldGrid, i: f64, j: f64) -> [f64; 2] {
    let [longitude, latitude] = field.projection.lon_lat_at_grid(i, j);
    let x = longitude.mul_add(1.0 / 360.0, 0.5);
    let y = (1.0 - latitude.to_radians().tan().asinh() / std::f64::consts::PI) * 0.5;
    [x, y]
}
