use crate::{
    cache::CacheStore,
    decode,
    model::{BladeKey, LeadHour, Product, RunExtent, RunId},
    xdg::Lair,
};
use anyhow::{Context as _, Result, bail};
use jiff::Timestamp;
use std::{ops::RangeInclusive, path::PathBuf};

const ORIGIN: &str = "https://noaa-hrrr-bdp-pds.s3.amazonaws.com";
const DISCOVERY_DEPTH: u8 = 30;

pub struct Source {
    agent: ureq::Agent,
    cache: CacheStore,
}

impl Source {
    pub fn new(lair: &Lair) -> Self {
        Self {
            agent: ureq::Agent::new_with_defaults(),
            cache: lair.field_cache(),
        }
    }

    pub fn discover(&self, now: Timestamp) -> Result<RunExtent> {
        let head = RunId::hourly_at_or_before(now);
        for age in 0..=DISCOVERY_DEPTH {
            let run = head.hours_ago(age);
            let Ok(extent) = self.survey(run) else {
                continue;
            };
            let Ok(index) = self.index(run, LeadHour::ZERO) else {
                continue;
            };
            if Product::ALL
                .iter()
                .all(|product| select_range(&index, *product).is_ok())
            {
                return Ok(extent);
            }
        }
        bail!("no complete HRRR run found in the last {DISCOVERY_DEPTH} hours")
    }

    pub fn survey(&self, run: RunId) -> Result<RunExtent> {
        let prefix = data_prefix(run)?;
        let url = format!("{ORIGIN}/?list-type=2&prefix={prefix}");
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

    pub fn field_message(&self, key: BladeKey) -> Result<Vec<u8>> {
        let blade = field_blade(key)?;
        self.cache.resolve(
            &blade,
            |bytes| decode::validate(key, bytes).is_ok(),
            || {
                let index = self.index(key.run, key.lead)?;
                let range = select_range(&index, key.product)?;
                let url = data_url(key.run, key.lead)?;
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
    Ok(frame_chamber(key.run, key.lead)?.join(format!("{}.grib2", key.product.cache_name())))
}

fn index_blade(run: RunId, lead: LeadHour) -> Result<PathBuf> {
    Ok(frame_chamber(run, lead)?.join("surface.idx"))
}

fn data_url(run: RunId, lead: LeadHour) -> Result<String> {
    Ok(format!(
        "{ORIGIN}/{}{:02}.grib2",
        data_prefix(run)?,
        lead.get()
    ))
}

fn data_prefix(run: RunId) -> Result<String> {
    Ok(format!(
        "hrrr.{}/conus/hrrr.t{:02}z.wrfsfcf",
        run.date()?,
        run.cycle()?
    ))
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

fn select_range(index: &str, product: Product) -> Result<RangeInclusive<u64>> {
    let entries = index.lines().map(parse_entry).collect::<Result<Vec<_>>>()?;
    let Some((slot, entry)) = entries
        .iter()
        .enumerate()
        .find(|(_, entry)| product.index_match(entry.descriptor))
    else {
        bail!("{product:?} absent from HRRR surface index");
    };
    let Some(next) = entries.get(slot + 1) else {
        bail!("{product:?} is terminal in HRRR index; byte extent is unknown");
    };
    let Some(last) = next.offset.checked_sub(1) else {
        bail!("invalid zero-length byte range for {product:?}");
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
93:800:d=2026071912:TCDC:entire atmosphere:2 hour fcst:\n\
94:900:d=2026071912:DPT:2 m above ground:2 hour fcst:\n";

    #[test]
    fn qpf_selectors_sever_run_and_hour_accumulations() -> Result<()> {
        assert_eq!(select_range(INDEX, Product::QpfRun)?, 200..=259);
        assert_eq!(select_range(INDEX, Product::QpfHour)?, 300..=349);
        Ok(())
    }

    #[test]
    fn both_qpf_products_admit_the_zero_hour_field() -> Result<()> {
        const ZERO: &str = "84:200:d=2026071912:APCP:surface:0-0 day acc fcst:\n\
85:260:d=2026071912:WEASD:surface:0-0 day acc fcst:\n";
        assert_eq!(select_range(ZERO, Product::QpfRun)?, 200..=259);
        assert_eq!(select_range(ZERO, Product::QpfHour)?, 200..=259);
        Ok(())
    }

    #[test]
    fn exact_level_selectors_cut_the_right_messages() -> Result<()> {
        assert_eq!(select_range(INDEX, Product::Smoke)?, 350..=499);
        assert_eq!(select_range(INDEX, Product::Temperature)?, 500..=799);
        assert_eq!(select_range(INDEX, Product::CloudCover)?, 800..=899);
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
