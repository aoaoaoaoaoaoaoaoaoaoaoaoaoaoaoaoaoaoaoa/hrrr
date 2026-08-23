use crate::model::{BladeKey, FieldGrid, GribTimeLaw, LambertGrid, VectorFrame};
use anyhow::{Context as _, Result, bail};
use grib::{
    Grib2SubmessageDecoder, GridDefinitionTemplateValues, Name, codetables::grib2::Table4_4,
};
use jiff::{Timestamp, tz::TimeZone};

pub fn field(key: BladeKey, bytes: &[u8]) -> Result<FieldGrid> {
    validate(key, bytes)?;
    let index_grib = grib::from_bytes(bytes).context("parse GRIB grid definition")?;
    let (_, index_message) = index_grib
        .iter()
        .next()
        .context("GRIB blade contains no submessage")?;
    let (width, height) = index_message.grid_shape().context("read GRIB grid shape")?;
    let indices = index_message.ij().context("read GRIB scanning law")?;
    let (projection, vector_frame) = projection(key, &index_message)?;

    let value_grib = grib::from_bytes(bytes).context("parse GRIB values")?;
    let (_, value_message) = value_grib
        .iter()
        .next()
        .context("GRIB blade contains no value submessage")?;
    let decoder = Grib2SubmessageDecoder::from(value_message).context("forge GRIB decoder")?;
    let values = decoder.dispatch().context("decode GRIB field")?;
    let mut grid = vec![f32::NAN; width.saturating_mul(height)];
    let mut decoded = 0_usize;
    for ((i, j), value) in indices.zip(values) {
        let Some(cell) = grid.get_mut(j * width + i) else {
            bail!("GRIB scanning law escaped {width}×{height} grid at ({i}, {j})");
        };
        *cell = value;
        decoded += 1;
    }
    if decoded != grid.len() {
        bail!(
            "GRIB decoder yielded {decoded} of {} grid points",
            grid.len()
        );
    }
    FieldGrid::forge_blade(grid, width, height, projection, vector_frame)
}

pub fn validate(key: BladeKey, bytes: &[u8]) -> Result<()> {
    let grib = grib::from_bytes(bytes).context("parse GRIB identity")?;
    let mut messages = grib.iter();
    let (_, message) = messages
        .next()
        .context("GRIB blade contains no submessage")?;
    if messages.next().is_some() {
        bail!("GRIB blade contains more than one submessage");
    }
    validate_identity(key, &message)
}

fn validate_identity<R: std::io::Read>(
    key: BladeKey,
    message: &grib::SubMessage<'_, R>,
) -> Result<()> {
    let law = key
        .grib_law()
        .context("blade key escaped its product recipe")?;
    let product = message.prod_def();
    if message.indicator().discipline != 0
        || product.prod_tmpl_num() != law.template
        || product.parameter_category() != Some(law.category)
        || product.parameter_number() != Some(law.parameter)
    {
        bail!(
            "GRIB product identity does not match requested {:?}",
            key.product
        );
    }
    let Some((surface, abyss)) = product.fixed_surfaces() else {
        bail!("GRIB product has no fixed-surface identity");
    };
    if surface.surface_type != law.surface.kind
        || surface.scale_factor != law.surface.scale_factor
        || surface.scaled_value != law.surface.scaled_value
        || abyss.surface_type != 255
    {
        bail!(
            "GRIB fixed surface does not match requested {:?}",
            key.product
        );
    }

    let reference = message.identification().ref_time_unchecked();
    validate_time(
        key.run.id.timestamp()?,
        [
            reference.year,
            u16::from(reference.month),
            u16::from(reference.day),
            u16::from(reference.hour),
            u16::from(reference.minute),
            u16::from(reference.second),
        ],
        "reference",
    )?;

    let expected_start = u32::from_be_bytes(i32::from(key.forecast_start()?).to_be_bytes());
    let Some(grib::ForecastTime {
        unit: Name(Table4_4::Hour),
        value,
    }) = product.forecast_time()
    else {
        bail!("GRIB forecast time is not expressed in hours");
    };
    if value != expected_start {
        bail!(
            "GRIB forecast begins at F{value:02}, requested {:?} {}",
            key.product,
            key.lead
        );
    }
    if law.time != GribTimeLaw::Instant {
        validate_interval(key, product.iter().copied().collect::<Vec<_>>().as_slice())?;
    }
    Ok(())
}

fn validate_interval(key: BladeKey, product: &[u8]) -> Result<()> {
    const END: std::ops::Range<usize> = 29..36;
    const RANGE_COUNT: usize = 36;
    const STATISTICAL_PROCESS: usize = 41;
    const RANGE_UNIT: usize = 43;
    const RANGE_LENGTH: std::ops::Range<usize> = 44..48;

    if product.len() < 53 {
        bail!("GRIB accumulation template is truncated");
    }
    validate_time(
        key.interval_end()?,
        [
            u16::from_be_bytes(product[END.start..END.start + 2].try_into()?),
            u16::from(product[END.start + 2]),
            u16::from(product[END.start + 3]),
            u16::from(product[END.start + 4]),
            u16::from(product[END.start + 5]),
            u16::from(product[END.start + 6]),
        ],
        "accumulation end",
    )?;
    let law = key
        .grib_law()
        .context("blade key escaped its product recipe")?;
    let (expected_process, expected_span) = match law.time {
        GribTimeLaw::AccumulationFromRun => (1, u32::from(key.lead.get())),
        GribTimeLaw::HourlyAccumulation => (1, u32::from(key.lead.get().min(1))),
        GribTimeLaw::DailySummary { .. } => (0, 23),
        GribTimeLaw::Instant => (0, 0),
    };
    let span = u32::from_be_bytes(product[RANGE_LENGTH].try_into()?);
    if product[RANGE_COUNT] != 1
        || product[STATISTICAL_PROCESS] != expected_process
        || product[RANGE_UNIT] != 1
        || span != expected_span
    {
        bail!(
            "GRIB statistical interval {span} h does not match requested {:?} {}",
            key.product,
            key.lead
        );
    }
    Ok(())
}

fn validate_time(expected: Timestamp, actual: [u16; 6], role: &str) -> Result<()> {
    let expected = expected.to_zoned(TimeZone::UTC);
    let wanted = [
        u16::try_from(expected.year())?,
        u16::try_from(expected.month())?,
        u16::try_from(expected.day())?,
        u16::try_from(expected.hour())?,
        u16::try_from(expected.minute())?,
        u16::try_from(expected.second())?,
    ];
    if actual != wanted {
        bail!("GRIB {role} time {actual:?} does not match requested {wanted:?}");
    }
    Ok(())
}

fn projection<R: std::io::Read>(
    key: BladeKey,
    message: &grib::SubMessage<'_, R>,
) -> Result<(LambertGrid, Option<VectorFrame>)> {
    let definition = GridDefinitionTemplateValues::try_from(message.grid_def())
        .context("decode GRIB grid definition")?;
    let GridDefinitionTemplateValues::Template30(lambert) = definition else {
        bail!("forecast field does not use Lambert conformal template 3.30");
    };
    let vector_frame = key.is_vector_component().then_some({
        if lambert.resolution_and_component_flags.0 & 0b0000_1000 == 0 {
            VectorFrame::Earth
        } else {
            VectorFrame::Grid
        }
    });
    let (major, minor) = lambert
        .earth_shape
        .radii()
        .context("GRIB grid has no defined earth radius")?;
    if (major - minor).abs() > 0.01 {
        bail!("ellipsoidal Lambert grids are not supported");
    }
    let degrees = |microdegrees: i32| f64::from(microdegrees) * 1.0e-6;
    let unsigned_degrees = |microdegrees: u32| f64::from(microdegrees) * 1.0e-6;
    let projection = LambertGrid::forge(
        major,
        degrees(lambert.first_point_lat),
        unsigned_degrees(lambert.first_point_lon),
        degrees(lambert.lad),
        unsigned_degrees(lambert.lov),
        [degrees(lambert.latin1), degrees(lambert.latin2)],
        [
            f64::from(lambert.dx) * 1.0e-3,
            f64::from(lambert.dy) * 1.0e-3,
        ],
    )?;
    Ok((projection, vector_frame))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ForecastRun, Ingredient, LeadHour, Product, RunId};

    #[test]
    fn grib_identity_is_bound_to_every_frame_axis() -> Result<()> {
        let run = RunId::forge(1_785_272_400)?;
        let lead = LeadHour::forge(20)?;
        for product in Product::ALL {
            let run = ForecastRun::forge(
                product.system(),
                product.system().cycle_at_or_before(run.timestamp()?)?,
            );
            for &ingredient in product.ingredients() {
                let key = BladeKey::forge(run, lead, product, ingredient)
                    .context("lawful product ingredient")?;
                let blade = synthetic_blade(key)?;
                validate(key, &blade)?;
                let wrong_lead = BladeKey::forge(
                    run,
                    if product == Product::AirQuality {
                        LeadHour::ZERO
                    } else {
                        LeadHour::forge(19)?
                    },
                    product,
                    ingredient,
                )
                .context("lawful wrong-lead ingredient")?;
                assert!(validate(wrong_lead, &blade).is_err());
                let wrong_run = BladeKey::forge(run.previous()?, lead, product, ingredient)
                    .context("lawful wrong-run ingredient")?;
                assert!(validate(wrong_run, &blade).is_err());
                let alien = if product == Product::Smoke {
                    Product::Temperature
                } else {
                    Product::Smoke
                };
                if let Some(wrong_product) = BladeKey::forge(run, lead, alien, Ingredient::Scalar) {
                    assert!(validate(wrong_product, &blade).is_err());
                }
            }
        }
        Ok(())
    }

    fn synthetic_blade(key: BladeKey) -> Result<Vec<u8>> {
        let reference = key.run.id.timestamp()?.to_zoned(TimeZone::UTC);
        let mut identification = vec![0_u8; 16];
        identification[0..2].copy_from_slice(&7_u16.to_be_bytes());
        identification[6] = 1;
        stamp(&mut identification[7..14], &reference)?;

        let law = key.grib_law().context("lawful synthetic blade recipe")?;
        let mut product = match law.time {
            GribTimeLaw::Instant => vec![0_u8; 29],
            GribTimeLaw::AccumulationFromRun
            | GribTimeLaw::HourlyAccumulation
            | GribTimeLaw::DailySummary { .. } => vec![0_u8; 53],
        };
        product[2..4].copy_from_slice(&law.template.to_be_bytes());
        product[4] = law.category;
        product[5] = law.parameter;
        product[12] = 1;
        let start = u32::from_be_bytes(i32::from(key.forecast_start()?).to_be_bytes());
        product[13..17].copy_from_slice(&start.to_be_bytes());
        product[17] = law.surface.kind;
        product[18] = law.surface.scale_factor as u8;
        product[19..23].copy_from_slice(&law.surface.scaled_value.to_be_bytes());
        product[23] = 255;
        if law.time != GribTimeLaw::Instant {
            let valid = key.interval_end()?.to_zoned(TimeZone::UTC);
            stamp(&mut product[29..36], &valid)?;
            product[36] = 1;
            product[41] = u8::from(!matches!(law.time, GribTimeLaw::DailySummary { .. }));
            product[42] = 2;
            product[43] = 1;
            let span = match law.time {
                GribTimeLaw::AccumulationFromRun => u32::from(key.lead.get()),
                GribTimeLaw::HourlyAccumulation => u32::from(key.lead.get().min(1)),
                GribTimeLaw::DailySummary { .. } => 23,
                GribTimeLaw::Instant => 0,
            };
            product[44..48].copy_from_slice(&span.to_be_bytes());
            product[48] = 255;
        }

        let mut blade = vec![0_u8; 16];
        blade[0..4].copy_from_slice(b"GRIB");
        blade[7] = 2;
        push_section(&mut blade, 1, &identification)?;
        push_section(&mut blade, 3, &[0, 0, 0, 0, 0, 0, 0, 0, 30])?;
        push_section(&mut blade, 4, &product)?;
        push_section(&mut blade, 5, &[0; 6])?;
        push_section(&mut blade, 6, &[255])?;
        push_section(&mut blade, 7, &[])?;
        blade.extend_from_slice(b"7777");
        let length = u64::try_from(blade.len())?;
        blade[8..16].copy_from_slice(&length.to_be_bytes());
        Ok(blade)
    }

    fn push_section(blade: &mut Vec<u8>, number: u8, payload: &[u8]) -> Result<()> {
        let length = u32::try_from(payload.len().saturating_add(5))?;
        blade.extend_from_slice(&length.to_be_bytes());
        blade.push(number);
        blade.extend_from_slice(payload);
        Ok(())
    }

    fn stamp(target: &mut [u8], time: &jiff::Zoned) -> Result<()> {
        target[0..2].copy_from_slice(&u16::try_from(time.year())?.to_be_bytes());
        target[2] = u8::try_from(time.month())?;
        target[3] = u8::try_from(time.day())?;
        target[4] = u8::try_from(time.hour())?;
        target[5] = u8::try_from(time.minute())?;
        target[6] = u8::try_from(time.second())?;
        Ok(())
    }
}
