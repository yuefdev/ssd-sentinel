//! Local, offline surface intelligence ("AI" triage) for the pinpoint repair engine.
//!
//! Deterministic statistics + expert rules, no network and no model download:
//! * a streaming latency baseline gives every disk its own adaptive "slow" threshold,
//! * scan findings become typed targets (unreadable sector / weak sector / slow area),
//! * dense clusters of bad sectors are widened into dead zones with a safety margin,
//! * every target gets an action (heal, isolate, relocate) and the disk gets a verdict.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Most targets kept from one full scan (enough for heavy damage, small enough for SQLite).
pub const MAX_TARGETS: usize = 8192;
/// Hits closer than this are treated as one damaged area.
pub const DEAD_ZONE_GAP: u64 = 64 * 1024 * 1024;
/// Hits needed inside one area before it is called a dead zone.
pub const DEAD_ZONE_MIN_HITS: usize = 3;
/// Extra space quarantined around a dead zone (damage on platters tends to spread).
pub const DEAD_ZONE_MARGIN: u64 = 1024 * 1024;
/// Largest single dead zone; anything bigger means "replace the disk", not "fence it off".
pub const DEAD_ZONE_MAX: u64 = 512 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    /// Read-only slow region (stale SSD data or a weak HDD area without a single bad sector).
    SlowArea,
    /// Sector that reads, but only after long retries.
    WeakSector,
    /// Sector that cannot be read at all (pending / uncorrectable).
    Unreadable,
}

impl TargetKind {
    pub fn label(self) -> &'static str {
        match self {
            TargetKind::SlowArea => "Yavaş bölge",
            TargetKind::WeakSector => "Zayıf sektör",
            TargetKind::Unreadable => "Okunamayan sektör",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Target {
    /// Byte offset on the physical disk.
    pub offset: u64,
    pub len: u64,
    pub kind: TargetKind,
    pub latency_ms: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Move user data away, rewrite the sector so firmware refreshes or remaps it, re-test.
    Heal,
    /// Move user data away and permanently fence the area off.
    Isolate,
    /// Rewrite user data to a fresh location (SSD stale-data refresh).
    Relocate,
}

/// Streaming latency histogram (log2 buckets from 1/16 ms) with an O(1) median.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LatencyBaseline {
    buckets: [u64; 32],
    count: u64,
}

impl LatencyBaseline {
    pub fn new() -> Self {
        Self::default()
    }

    fn bucket(ms: f64) -> usize {
        let v = (ms / 0.0625).max(1.0);
        (v.log2().floor() as usize).min(31)
    }

    pub fn push(&mut self, ms: f64) {
        if ms.is_finite() && ms >= 0.0 {
            self.buckets[Self::bucket(ms)] += 1;
            self.count += 1;
        }
    }

    pub fn count(&self) -> u64 {
        self.count
    }

    /// Geometric centre of the bucket holding the median sample.
    pub fn median(&self) -> Option<f64> {
        if self.count == 0 {
            return None;
        }
        let half = self.count.div_ceil(2);
        let mut seen = 0;
        for (i, n) in self.buckets.iter().enumerate() {
            seen += n;
            if seen >= half {
                return Some(0.0625 * 2f64.powi(i as i32) * std::f64::consts::SQRT_2);
            }
        }
        None
    }
}

/// Per-disk "slow chunk" threshold for 1 MiB reads. Fixed until 256 samples exist, then
/// a multiple of this disk's own median, so a USB HDD and an NVMe SSD are judged fairly.
pub fn slow_threshold_ms(base: &LatencyBaseline, is_hdd: bool) -> f64 {
    if base.count() < 256 {
        return if is_hdd { 80.0 } else { 30.0 };
    }
    let m = base.median().unwrap_or(1.0);
    if is_hdd {
        (m * 8.0).clamp(50.0, 150.0)
    } else {
        (m * 10.0).clamp(8.0, 80.0)
    }
}

/// Groups unreadable / weak hits that sit close together. Returns physical `(start, end)`
/// spans (margin included) for every group with at least `DEAD_ZONE_MIN_HITS` hits.
pub fn dead_zones(targets: &[Target]) -> Vec<(u64, u64)> {
    let mut hits: Vec<&Target> = targets
        .iter()
        .filter(|t| t.kind != TargetKind::SlowArea)
        .collect();
    hits.sort_by_key(|t| t.offset);
    let mut zones = Vec::new();
    let mut i = 0;
    while i < hits.len() {
        let mut j = i;
        while j + 1 < hits.len()
            && hits[j + 1]
                .offset
                .saturating_sub(hits[j].offset + hits[j].len)
                <= DEAD_ZONE_GAP
            && hits[j + 1].offset + hits[j + 1].len - hits[i].offset <= DEAD_ZONE_MAX
        {
            j += 1;
        }
        if j + 1 - i >= DEAD_ZONE_MIN_HITS {
            let start = hits[i].offset.saturating_sub(DEAD_ZONE_MARGIN);
            let end = hits[j].offset + hits[j].len + DEAD_ZONE_MARGIN;
            zones.push((start, end));
        }
        i = j + 1;
    }
    zones
}

/// One planned repair unit in volume cluster space.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Region {
    pub lcn: u64,
    pub clusters: u64,
    pub phys_offset: u64,
    pub kind: TargetKind,
    pub dead_zone: bool,
    pub latency_ms: Option<f64>,
}

/// Maps physical targets onto the selected volume, widens dead zones, merges overlaps.
/// Returns the regions and how many targets fell outside this volume.
pub fn plan_regions(
    targets: &[Target],
    vol_start: u64,
    vol_len: u64,
    cluster: u64,
) -> (Vec<Region>, usize) {
    let cluster = cluster.max(512);
    let vol_end = vol_start.saturating_add(vol_len);
    let zones = dead_zones(targets);
    let mut spans: Vec<(u64, u64, TargetKind, bool, Option<f64>)> = Vec::new();
    let mut outside = 0usize;
    for t in targets {
        let end = t.offset + t.len.max(1);
        if end <= vol_start || t.offset >= vol_end {
            outside += 1;
            continue;
        }
        let in_zone = zones.iter().any(|(s, e)| t.offset < *e && end > *s);
        if !in_zone {
            spans.push((
                t.offset.max(vol_start),
                end.min(vol_end),
                t.kind,
                false,
                t.latency_ms,
            ));
        }
    }
    for (s, e) in &zones {
        if *e <= vol_start || *s >= vol_end {
            continue;
        }
        let worst = targets
            .iter()
            .filter(|t| t.offset < *e && t.offset + t.len > *s)
            .map(|t| t.kind)
            .max()
            .unwrap_or(TargetKind::WeakSector);
        let lat = targets
            .iter()
            .filter(|t| t.offset < *e && t.offset + t.len > *s)
            .filter_map(|t| t.latency_ms)
            .fold(None, |acc: Option<f64>, v| {
                Some(acc.map_or(v, |a| a.max(v)))
            });
        spans.push(((*s).max(vol_start), (*e).min(vol_end), worst, true, lat));
    }
    let mut regions: Vec<Region> = spans
        .into_iter()
        .map(|(s, e, kind, dz, lat)| {
            let lcn = (s - vol_start) / cluster;
            let end_lcn = (e - vol_start).div_ceil(cluster);
            Region {
                lcn,
                clusters: end_lcn.saturating_sub(lcn).max(1),
                phys_offset: vol_start + lcn * cluster,
                kind,
                dead_zone: dz,
                latency_ms: lat,
            }
        })
        .collect();
    regions.sort_by_key(|r| r.lcn);
    let mut merged: Vec<Region> = Vec::with_capacity(regions.len());
    for r in regions {
        if let Some(last) = merged.last_mut() {
            if r.lcn <= last.lcn + last.clusters {
                let end = (r.lcn + r.clusters).max(last.lcn + last.clusters);
                last.clusters = end - last.lcn;
                last.kind = last.kind.max(r.kind);
                last.dead_zone |= r.dead_zone;
                last.latency_ms = match (last.latency_ms, r.latency_ms) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    (a, b) => a.or(b),
                };
                continue;
            }
        }
        merged.push(r);
    }
    (merged, outside)
}

/// What the engine should do with one region.
pub fn decide(region: &Region, is_hdd: bool, isolate_mode: bool) -> Action {
    if !is_hdd {
        return Action::Relocate;
    }
    if isolate_mode || region.dead_zone {
        Action::Isolate
    } else {
        Action::Heal
    }
}

/// Disk-level verdict after a full scan, shown as the "Akıllı teşhis" card.
pub fn diagnose(targets: &[Target], is_hdd: bool, pending_197: u64, tail_degraded: bool) -> Value {
    let count = |k: TargetKind| targets.iter().filter(|t| t.kind == k).count();
    let unreadable = count(TargetKind::Unreadable);
    let weak = count(TargetKind::WeakSector);
    let slow = count(TargetKind::SlowArea);
    let zones = if is_hdd { dead_zones(targets).len() } else { 0 };
    let penalty = (unreadable * 2).min(60)
        + weak.min(25)
        + (slow / 4).min(15)
        + zones * 5
        + if pending_197 > 0 { 10 } else { 0 }
        + if tail_degraded { 5 } else { 0 };
    let score = 100usize.saturating_sub(penalty);
    let (verdict, title, advice, recommended): (&str, &str, String, Option<&str>) = if unreadable
        + weak
        + slow
        == 0
    {
        if pending_197 > 0 {
            (
                    "attention",
                    "Yüzey temiz, ama diskte bekleyen sektör kaydı var",
                    format!("Tarama hiçbir bozuk okuma bulmadı; SMART {pending_197} bekleyen sektör bildiriyor. Bunlar büyük olasılıkla boş alanda; bir sonraki yazmada disk kendisi düzeltir."),
                    None,
                )
        } else {
            (
                "healthy",
                "Yüzey temiz",
                "Bozuk, zayıf veya yavaş bölge bulunmadı. Onarım gerekmez.".into(),
                None,
            )
        }
    } else if is_hdd && (unreadable > 500 || zones > 10) {
        (
                "failing",
                "Disk ciddi hasarlı",
                format!("{unreadable} okunamayan sektör ve {zones} ölü bölge var. Önce verilerinizi başka diske yedekleyin, sonra ölü bölgeleri kapatın ve diski değiştirmeyi planlayın."),
                Some("bad_zone_isolate"),
            )
    } else if !is_hdd {
        (
                "repairable",
                "Yavaş okunan bölgeler bulundu",
                format!("{} yavaş bölge var (genelde uzun süredir yazılmamış eski veri). Tazeleme bu verileri yeni hücrelere yazarak okuma hızını geri getirir.", slow + weak + unreadable),
                Some("magnetic_refresh"),
            )
    } else if zones > 0 {
        (
                "repairable",
                "Yoğun hasarlı bölge bulundu",
                format!("Bozuk sektörler {zones} bölgede kümelenmiş. Bu alanlardaki dosyalar taşınıp bölge kalıcı olarak kapatılmalı; dağınık sektörler ayrıca onarılabilir."),
                Some("bad_zone_isolate"),
            )
    } else {
        (
                "repairable",
                "Onarılabilir sektörler bulundu",
                format!("{unreadable} okunamayan ve {weak} zayıf sektör dağınık halde. Nokta atışı onarım bunları yeniden yazıp diskin yedek sektörlere aktarmasını sağlar."),
                Some("magnetic_refresh"),
            )
    };
    json!({
        "score": score,
        "verdict": verdict,
        "title": title,
        "advice": advice,
        "recommended_mode": recommended,
        "unreadable": unreadable,
        "weak": weak,
        "slow": slow,
        "dead_zones": zones,
        "engine": "Yerel akıllı teşhis · istatistik + kural tabanlı · çevrimdışı"
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;

    fn t(offset: u64, kind: TargetKind) -> Target {
        Target {
            offset,
            len: 4096,
            kind,
            latency_ms: None,
        }
    }

    #[test]
    fn baseline_median_and_adaptive_threshold_follow_the_disk() {
        let mut nvme = LatencyBaseline::new();
        for _ in 0..500 {
            nvme.push(0.6);
        }
        let ssd = slow_threshold_ms(&nvme, false);
        assert!((8.0..=10.0).contains(&ssd), "{ssd}");
        let mut hdd = LatencyBaseline::new();
        for _ in 0..500 {
            hdd.push(9.0);
        }
        let h = slow_threshold_ms(&hdd, true);
        assert!((50.0..=150.0).contains(&h), "{h}");
        assert_eq!(slow_threshold_ms(&LatencyBaseline::new(), true), 80.0);
    }

    #[test]
    fn dense_bad_sectors_become_one_dead_zone_with_margin() {
        let targets = vec![
            t(100 * MIB, TargetKind::Unreadable),
            t(110 * MIB, TargetKind::WeakSector),
            t(130 * MIB, TargetKind::Unreadable),
            t(900 * MIB, TargetKind::Unreadable),
        ];
        let zones = dead_zones(&targets);
        assert_eq!(zones.len(), 1);
        assert_eq!(zones[0].0, 99 * MIB);
        assert_eq!(zones[0].1, 130 * MIB + 4096 + MIB);
    }

    #[test]
    fn plan_maps_to_clusters_merges_and_skips_other_volumes() {
        let vol_start = 200 * MIB;
        let targets = vec![
            t(vol_start + 8192, TargetKind::WeakSector),
            t(vol_start + 8192 + 4096, TargetKind::Unreadable),
            t(MIB, TargetKind::Unreadable),
        ];
        let (regions, outside) = plan_regions(&targets, vol_start, 100 * MIB, 4096);
        assert_eq!(outside, 1);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].lcn, 2);
        assert_eq!(regions[0].clusters, 2);
        assert_eq!(regions[0].kind, TargetKind::Unreadable);
        assert_eq!(regions[0].phys_offset, vol_start + 8192);
        assert_eq!(decide(&regions[0], true, false), Action::Heal);
        assert_eq!(decide(&regions[0], true, true), Action::Isolate);
        assert_eq!(decide(&regions[0], false, false), Action::Relocate);
    }

    #[test]
    fn diagnosis_recommends_the_right_repair() {
        let clean = diagnose(&[], true, 0, false);
        assert_eq!(clean["verdict"], "healthy");
        assert!(clean["recommended_mode"].is_null());
        let scattered = diagnose(
            &[
                t(0, TargetKind::Unreadable),
                t(10_000 * MIB, TargetKind::WeakSector),
            ],
            true,
            1,
            false,
        );
        assert_eq!(scattered["recommended_mode"], "magnetic_refresh");
        let dense: Vec<_> = (0..4).map(|i| t(i * MIB, TargetKind::Unreadable)).collect();
        assert_eq!(
            diagnose(&dense, true, 0, false)["recommended_mode"],
            "bad_zone_isolate"
        );
        let stale = vec![Target {
            offset: 0,
            len: MIB,
            kind: TargetKind::SlowArea,
            latency_ms: Some(40.0),
        }];
        assert_eq!(
            diagnose(&stale, false, 0, false)["recommended_mode"],
            "magnetic_refresh"
        );
    }

    #[test]
    fn latency_baseline_serde_roundtrip_preserves_buckets_and_threshold() {
        let mut base = LatencyBaseline::new();
        for i in 0..300 {
            base.push(if i % 5 == 0 { 12.0 } else { 8.5 });
        }
        let encoded = serde_json::to_value(&base).unwrap();
        let decoded: LatencyBaseline = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, base);
        assert_eq!(decoded.count(), 300);
        assert_eq!(decoded.median(), base.median());
        assert_eq!(
            slow_threshold_ms(&decoded, true),
            slow_threshold_ms(&base, true)
        );
    }
}
