//! Stable, world-anchored wind barbs stamped through app-owned dies.

use crate::{
    map,
    model::{FieldGrid, Viewport},
};
use brass_poolrooms::chrome::{ForgedMesh, ForgedVertex};
use std::f32::consts::TAU;

mod baked {
    use super::{ForgedMesh, ForgedVertex};

    include!(concat!(env!("OUT_DIR"), "/wind_barb.rs"));
}

const TARGET_PITCH: f32 = 48.0;
const METRES_PER_SECOND_TO_KNOTS: f32 = 1.943_844_6;
const STAFF_TIP: f32 = -32.0;
const PENNANT_PITCH: f32 = 6.0;
const FEATHER_INSET: f32 = 6.0;
const FEATHER_PITCH: f32 = 3.8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Magnitude {
    Calm,
    Barb { pennants: u8, full: u8, half: bool },
}

impl Magnitude {
    fn from_metres_per_second(speed: f32) -> Self {
        let quanta = (speed * METRES_PER_SECOND_TO_KNOTS / 5.0)
            .round()
            .clamp(0.0, 40.0) as u8;
        if quanta == 0 {
            Self::Calm
        } else {
            let remainder = quanta % 10;
            Self::Barb {
                pennants: quanta / 10,
                full: remainder / 2,
                half: !remainder.is_multiple_of(2),
            }
        }
    }
}

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
            let anchor = map::screen_at(view, rect, world_at_grid(field, i, j));
            if !rect.contains(anchor) {
                continue;
            }
            match Magnitude::from_metres_per_second(eastward.hypot(northward)) {
                Magnitude::Calm => baked::CALM.stamp(&mut mesh, anchor),
                Magnitude::Barb {
                    pennants,
                    full,
                    half,
                } => {
                    let angle = (-eastward).atan2(-northward).rem_euclid(TAU);
                    let pose = ((angle / TAU * baked::POSE_COUNT as f32).round() as usize)
                        % baked::POSE_COUNT;
                    baked::STAFF[pose].stamp(&mut mesh, anchor);
                    paint_feathers(&mut mesh, anchor, angle, pose, pennants, full, half);
                }
            }
        }
    }
    if !mesh.indices.is_empty() {
        let _barbs = painter.add(egui::Shape::mesh(mesh));
    }
}

fn paint_feathers(
    mesh: &mut egui::Mesh,
    anchor: egui::Pos2,
    angle: f32,
    pose: usize,
    pennants: u8,
    full: u8,
    half: bool,
) {
    let mut distance = STAFF_TIP;
    for _ in 0..pennants {
        baked::PENNANT[pose].stamp(mesh, displaced(anchor, angle, distance));
        distance += PENNANT_PITCH;
    }
    distance += FEATHER_INSET;
    for _ in 0..full {
        baked::FULL[pose].stamp(mesh, displaced(anchor, angle, distance));
        distance += FEATHER_PITCH;
    }
    if half {
        baked::HALF[pose].stamp(mesh, displaced(anchor, angle, distance));
    }
}

fn displaced(anchor: egui::Pos2, angle: f32, distance: f32) -> egui::Pos2 {
    let (sin, cos) = angle.sin_cos();
    anchor + egui::vec2(-distance * sin, distance * cos)
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
    let cell = center
        .distance(east)
        .midpoint(center.distance(north))
        .max(0.01);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_barb_quanta_compose_flags_and_feathers() {
        let from_knots =
            |knots| Magnitude::from_metres_per_second(knots / METRES_PER_SECOND_TO_KNOTS);
        assert_eq!(from_knots(2.0), Magnitude::Calm);
        assert_eq!(
            from_knots(5.0),
            Magnitude::Barb {
                pennants: 0,
                full: 0,
                half: true,
            }
        );
        assert_eq!(
            from_knots(65.0),
            Magnitude::Barb {
                pennants: 1,
                full: 1,
                half: true,
            }
        );
    }
}
