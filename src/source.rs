use crate::{
    application_paths::ApplicationPaths,
    cache::CacheStore,
    decode,
    model::{
        AqmBundle, BladeKey, ForecastRun, ForecastSystem, LeadHour, Product, RunExtent, RunId,
    },
};
use anyhow::{Context as _, Result, bail};
use jiff::Timestamp;
use std::{ops::RangeInclusive, path::PathBuf};

const HRRR_ORIGIN: &str = "https://noaa-hrrr-bdp-pds.s3.amazonaws.com";
const AQM_ORIGIN: &str = "https://nomads.ncep.noaa.gov/pub/data/nccf/com/aqm/prod";
const HRRR_DISCOVERY_DEPTH: u8 = 30;
const AQM_DISCOVERY_DEPTH: u8 = 6;

pub struct Source {
    agent: ureq::Agent,
    cache: CacheStore,
}

impl Source {
    pub fn new(paths: &ApplicationPaths) -> Self {
        Self {
            agent: ureq::Agent::new_with_defaults(),
            cache: paths.field_cache(),
        }
    }

    pub fn discover(&self, system: ForecastSystem, now: Timestamp) -> Result<RunExtent> {
        match system {
            ForecastSystem::Hrrr => self.discover_hrrr(now),
            ForecastSystem::Aqm => self.discover_aqm(now),
        }
    }

    pub fn survey(&self, run: ForecastRun) -> Result<RunExtent> {
        match run.system {
            ForecastSystem::Hrrr => self.survey_hrrr(run),
            ForecastSystem::Aqm => self.survey_aqm(run),
        }
    }

    fn discover_hrrr(&self, now: Timestamp) -> Result<RunExtent> {
        let head = ForecastSystem::Hrrr.cycle_at_or_before(now)?;
        for age in 0..=HRRR_DISCOVERY_DEPTH {
            let run = ForecastRun::hrrr(head.hours_ago(age));
            let Ok(extent) = self.survey_hrrr(run) else {
                continue;
            };
            let Ok(index) = self.index(run.id, LeadHour::ZERO) else {
                continue;
            };
            let complete = Product::ALL
                .iter()
                .copied()
                .filter(|product| product.system() == ForecastSystem::Hrrr)
                .all(|product| {
                    product.ingredients().iter().all(|&ingredient| {
                        BladeKey::forge(run, LeadHour::ZERO, product, ingredient)
                            .is_some_and(|key| select_range(&index, key).is_ok())
                    })
                });
            if complete {
                return Ok(extent);
            }
        }
        bail!("no complete HRRR run found in the last {HRRR_DISCOVERY_DEPTH} hours")
    }

    fn discover_aqm(&self, now: Timestamp) -> Result<RunExtent> {
        let mut run = ForecastRun::forge(
            ForecastSystem::Aqm,
            ForecastSystem::Aqm.cycle_at_or_before(now)?,
        );
        for _age in 0..=AQM_DISCOVERY_DEPTH {
            if let Ok(extent) = self.survey_aqm(run) {
                return Ok(extent);
            }
            run = run.previous()?;
        }
        bail!("no complete air-quality run found in the recent NOAA inventory")
    }

    fn survey_hrrr(&self, run: ForecastRun) -> Result<RunExtent> {
        let prefix = data_prefix(run)?;
        let url = format!("{HRRR_ORIGIN}/?list-type=2&prefix={prefix}");
        let listing = self
            .agent
            .get(&url)
            .call()
            .with_context(|| format!("survey {run:?} publication frontier"))?
            .body_mut()
            .read_to_string()
            .with_context(|| format!("read {run:?} publication inventory"))?;
        if listing.contains("<IsTruncated>true</IsTruncated>") {
            bail!("{run:?} publication inventory was unexpectedly truncated");
        }
        let horizon = run.horizon()?;
        let published = publication_frontier(&listing, &prefix, horizon)
            .with_context(|| format!("{run:?} has no contiguous published forecast prefix"))?;
        RunExtent::forge(run, published)
    }

    fn survey_aqm(&self, run: ForecastRun) -> Result<RunExtent> {
        for bundle in AqmBundle::ALL {
            let url = aqm_url(run, bundle)?;
            let _response = self
                .agent
                .head(&url)
                .call()
                .with_context(|| format!("survey air-quality bundle {url}"))?;
        }
        RunExtent::forge(run, run.horizon()?)
    }

    pub fn field_message(&self, key: BladeKey) -> Result<Vec<u8>> {
        match key.run.system {
            ForecastSystem::Hrrr => self.hrrr_field_message(key),
            ForecastSystem::Aqm => self.aqm_field_message(key),
        }
    }

    fn hrrr_field_message(&self, key: BladeKey) -> Result<Vec<u8>> {
        let blade = field_blade(key)?;
        self.cache.resolve(
            &blade,
            |bytes| decode::validate(key, bytes).is_ok(),
            || {
                let index = self.index(key.run.id, key.lead)?;
                let range = select_range(&index, key)?;
                let url = data_url(key.run.id, key.lead)?;
                let mut response = self
                    .agent
                    .get(&url)
                    .header("Range", format!("bytes={}-{}", range.start(), range.end()))
                    .call()
                    .with_context(|| format!("fetch {:?} {} from {url}", key.product, key.lead))?;
                let bytes = response
                    .body_mut()
                    .read_to_vec()
                    .with_context(|| format!("read {:?} {} body", key.product, key.lead))?;
                decode::validate(key, &bytes).with_context(|| {
                    format!(
                        "range {}-{} did not produce requested {:?} {}",
                        range.start(),
                        range.end(),
                        key.product,
                        key.lead
                    )
                })?;
                Ok(bytes)
            },
        )
    }

    fn aqm_field_message(&self, key: BladeKey) -> Result<Vec<u8>> {
        let blade = field_blade(key)?;
        self.cache.resolve(
            &blade,
            |bytes| decode::validate(key, bytes).is_ok(),
            || {
                let bundle = key.aqm_bundle().context("AQM blade has no bundle law")?;
                let bundle_blade = aqm_bundle_blade(key.run, bundle)?;
                let url = aqm_url(key.run, bundle)?;
                let bytes = self.cache.resolve(
                    &bundle_blade,
                    |bytes| grib_message(bytes, usize::from(AqmBundle::DAY_SLOTS - 1)).is_ok(),
                    || {
                        let bytes = self
                            .agent
                            .get(&url)
                            .call()
                            .with_context(|| format!("fetch {url}"))?
                            .body_mut()
                            .read_to_vec()
                            .with_context(|| format!("read {url}"))?;
                        let _last = grib_message(&bytes, usize::from(AqmBundle::DAY_SLOTS - 1))
                            .with_context(|| format!("{url} is not a three-day GRIB bundle"))?;
                        Ok(bytes)
                    },
                )?;
                let slot = usize::from(key.daily_slot().context("AQM blade has no day slot")?);
                let message = grib_message(&bytes, slot)?.to_vec();
                decode::validate(key, &message)
                    .with_context(|| format!("AQM bundle {url} did not match day slot {slot}"))?;
                Ok(message)
            },
        )
    }

    fn index(&self, run: RunId, lead: LeadHour) -> Result<String> {
        let blade = index_blade(run, lead)?;
        let url = format!("{}.idx", data_url(run, lead)?);
        let bytes = self.cache.resolve(
            &blade,
            |bytes| std::str::from_utf8(bytes).is_ok_and(valid_index),
            || {
                let index = self
                    .agent
                    .get(&url)
                    .call()
                    .with_context(|| format!("fetch {url}"))?
                    .body_mut()
                    .read_to_string()
                    .with_context(|| format!("read {url}"))?;
                if !valid_index(&index) {
                    bail!("{url} did not produce a valid HRRR index");
                }
                Ok(index.into_bytes())
            },
        )?;
        String::from_utf8(bytes).context("cached HRRR index ceased to be UTF-8")
    }
}

fn frame_chamber(run: RunId, lead: LeadHour) -> Result<PathBuf> {
    Ok(PathBuf::from(run.stamp()?).join(format!("f{:02}", lead.get())))
}

fn field_blade(key: BladeKey) -> Result<PathBuf> {
    let cache_name = key
        .cache_name()
        .context("blade key escaped its product recipe")?;
    let chamber = match key.run.system {
        ForecastSystem::Hrrr => frame_chamber(key.run.id, key.lead)?,
        ForecastSystem::Aqm => PathBuf::from("aqm").join(key.run.id.stamp()?).join(format!(
            "day-{}",
            key.daily_slot().context("AQM blade has no day slot")?
        )),
    };
    Ok(chamber.join(format!("{cache_name}.grib2")))
}

fn aqm_bundle_blade(run: ForecastRun, bundle: AqmBundle) -> Result<PathBuf> {
    Ok(PathBuf::from("aqm")
        .join(run.id.stamp()?)
        .join(format!("{}.grib2", bundle.file_stem())))
}

fn index_blade(run: RunId, lead: LeadHour) -> Result<PathBuf> {
    Ok(frame_chamber(run, lead)?.join("surface.idx"))
}

fn data_url(run: RunId, lead: LeadHour) -> Result<String> {
    Ok(format!(
        "{HRRR_ORIGIN}/{}{:02}.grib2",
        data_prefix(ForecastRun::hrrr(run))?,
        lead.get()
    ))
}

fn data_prefix(run: ForecastRun) -> Result<String> {
    Ok(format!(
        "hrrr.{}/conus/hrrr.t{:02}z.wrfsfcf",
        run.id.date()?,
        run.id.cycle()?
    ))
}

fn aqm_url(run: ForecastRun, bundle: AqmBundle) -> Result<String> {
    Ok(format!(
        "{AQM_ORIGIN}/aqm.{}/{:02}/aqm.t{:02}z.{}.227.grib2",
        run.id.date()?,
        run.id.cycle()?,
        run.id.cycle()?,
        bundle.file_stem(),
    ))
}

fn grib_message(bundle: &[u8], wanted: usize) -> Result<&[u8]> {
    let mut offset = 0_usize;
    let mut slot = 0_usize;
    while offset < bundle.len() {
        let header = bundle
            .get(offset..offset.saturating_add(16))
            .context("truncated GRIB bundle header")?;
        if &header[..4] != b"GRIB" {
            bail!("GRIB bundle lost message alignment at byte {offset}");
        }
        let length = usize::try_from(u64::from_be_bytes(header[8..16].try_into()?))?;
        let end = offset
            .checked_add(length)
            .context("GRIB message length overflow")?;
        let message = bundle
            .get(offset..end)
            .context("truncated GRIB bundle message")?;
        if message.get(length.saturating_sub(4)..) != Some(b"7777") {
            bail!("GRIB bundle message {slot} has no terminator");
        }
        if slot == wanted {
            return Ok(message);
        }
        offset = end;
        slot += 1;
    }
    bail!("GRIB bundle has no message at slot {wanted}")
}

fn publication_frontier(listing: &str, prefix: &str, horizon: LeadHour) -> Option<LeadHour> {
    let mut published = [false; LeadHour::MAX as usize + 1];
    for entry in listing.split("<Key>").skip(1) {
        let Some((key, _rest)) = entry.split_once("</Key>") else {
            continue;
        };
        let Some(hour) = key
            .strip_prefix(prefix)
            .and_then(|suffix| suffix.strip_suffix(".grib2.idx"))
            .filter(|hour| hour.len() == 2)
            .and_then(|hour| hour.parse::<u8>().ok())
            .filter(|hour| *hour <= horizon.get())
        else {
            continue;
        };
        published[usize::from(hour)] = true;
    }
    (0..=horizon.get())
        .take_while(|hour| published[usize::from(*hour)])
        .last()
        .and_then(|hour| LeadHour::forge(hour).ok())
}

#[derive(Debug)]
struct IndexEntry<'a> {
    offset: u64,
    descriptor: &'a str,
}

fn select_range(index: &str, key: BladeKey) -> Result<RangeInclusive<u64>> {
    let entries = index.lines().map(parse_entry).collect::<Result<Vec<_>>>()?;
    let Some((slot, entry)) = entries
        .iter()
        .enumerate()
        .find(|(_, entry)| key.index_match(entry.descriptor))
    else {
        bail!(
            "{:?} {:?} absent from HRRR surface index",
            key.product,
            key.ingredient()
        );
    };
    let Some(next) = entries.get(slot + 1) else {
        bail!(
            "{:?} {:?} is terminal in HRRR index; byte extent is unknown",
            key.product,
            key.ingredient()
        );
    };
    let Some(last) = next.offset.checked_sub(1) else {
        bail!("invalid zero-length byte range for {:?}", key.product);
    };
    Ok(entry.offset..=last)
}

fn parse_entry(line: &str) -> Result<IndexEntry<'_>> {
    let mut columns = line.splitn(3, ':');
    let record = columns.next().unwrap_or_default();
    let offset = columns
        .next()
        .with_context(|| format!("index record {record} has no offset"))?
        .parse::<u64>()
        .with_context(|| format!("index record {record} has invalid offset"))?;
    let descriptor = columns
        .next()
        .with_context(|| format!("index record {record} has no descriptor"))?;
    Ok(IndexEntry { offset, descriptor })
}

fn valid_index(index: &str) -> bool {
    !index.is_empty() && index.lines().all(|line| parse_entry(line).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    const INDEX: &str = "\
83:100:d=2026071912:PRATE:surface:2 hour fcst:\n\
84:200:d=2026071912:APCP:surface:0-2 hour acc fcst:\n\
85:260:d=2026071912:WEASD:surface:0-2 hour acc fcst:\n\
90:300:d=2026071912:APCP:surface:1-2 hour acc fcst:\n\
91:350:d=2026071912:MASSDEN:8 m above ground:2 hour fcst:\n\
92:500:d=2026071912:TMP:2 m above ground:2 hour fcst:\n\
93:800:d=2026071912:DPT:2 m above ground:2 hour fcst:\n\
94:900:d=2026071912:TCDC:entire atmosphere:2 hour fcst:\n\
95:1000:d=2026071912:UGRD:10 m above ground:2 hour fcst:\n\
96:1100:d=2026071912:VGRD:10 m above ground:2 hour fcst:\n\
97:1200:d=2026071912:VIS:surface:2 hour fcst:\n";

    #[test]
    fn inventory_selectors_distinguish_products_and_accumulation_windows() -> Result<()> {
        use crate::model::Ingredient::*;
        let run = RunId::forge(1_785_272_400)?;
        let run = ForecastRun::hrrr(run);
        let key = |product, ingredient| {
            BladeKey::forge(run, LeadHour::ZERO, product, ingredient).context("lawful test blade")
        };
        assert_eq!(
            select_range(INDEX, key(Product::QpfRun, Scalar)?)?,
            200..=259
        );
        assert_eq!(
            select_range(INDEX, key(Product::QpfHour, Scalar)?)?,
            300..=349
        );
        assert_eq!(
            select_range(INDEX, key(Product::Smoke, Scalar)?)?,
            350..=499
        );
        assert_eq!(
            select_range(INDEX, key(Product::Temperature, Scalar)?)?,
            500..=799
        );
        assert_eq!(
            select_range(INDEX, key(Product::DewPoint, Scalar)?)?,
            800..=899
        );
        assert_eq!(
            select_range(INDEX, key(Product::CloudCover, Scalar)?)?,
            900..=999
        );
        assert_eq!(
            select_range(INDEX, key(Product::Wind, Eastward)?)?,
            1000..=1099
        );
        assert_eq!(
            select_range(INDEX, key(Product::Wind, Northward)?)?,
            1100..=1199
        );

        const ZERO: &str = "84:200:d=2026071912:APCP:surface:0-0 day acc fcst:\n\
85:260:d=2026071912:WEASD:surface:0-0 day acc fcst:\n";
        assert_eq!(
            select_range(ZERO, key(Product::QpfRun, Scalar)?)?,
            200..=259
        );
        assert_eq!(
            select_range(ZERO, key(Product::QpfHour, Scalar)?)?,
            200..=259
        );
        Ok(())
    }

    #[test]
    fn publication_frontier_stops_before_holes_and_ignores_full_gribs() -> Result<()> {
        let prefix = "hrrr.20260720/conus/hrrr.t20z.wrfsfcf";
        let listing = format!(
            "<Key>{prefix}00.grib2</Key>\
             <Key>{prefix}00.grib2.idx</Key>\
             <Key>{prefix}01.grib2.idx</Key>\
             <Key>{prefix}03.grib2.idx</Key>"
        );
        assert_eq!(
            publication_frontier(&listing, prefix, LeadHour::forge(18)?),
            Some(LeadHour::forge(1)?)
        );
        Ok(())
    }
}
