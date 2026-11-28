//! Graphics cards from the per-process engine counters and the per-adapter
//! memory counters. Pure code, so it is tested everywhere.

use crate::{Gpu, GpuEngine};
use std::collections::HashMap;

/// An adapter LUID as one number: the high word above the low one.
pub(crate) type Luid = u64;

/// One engine of one card: adapter LUID, physical GPU and engine index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct Engine {
    luid: Luid,
    phys: u32,
    eng: u32,
}

/// The LUID in a name that holds `luid_0x00000000_0x0000D4A5`.
pub(crate) fn luid(name: &str) -> Option<Luid> {
    let lower = name.to_ascii_lowercase();
    let at = lower.find("luid_")? + "luid_".len();
    let mut parts = lower[at..].split('_');
    let hex = |s: &str| u32::from_str_radix(s.strip_prefix("0x")?, 16).ok();
    let hi = hex(parts.next()?)?;
    let lo = hex(parts.next()?)?;
    Some(Luid::from(hi) << 32 | Luid::from(lo))
}

/// The LUID as the counters and DXGI give it: the low word and the signed high word.
pub(crate) fn luid_from_parts(low: u32, high: i32) -> Luid {
    Luid::from(high.cast_unsigned()) << 32 | Luid::from(low)
}

/// Parses `pid_123_luid_0x00000000_0x0000D4A5_phys_0_eng_3_engtype_Compute 0`
/// into the engine and the kind of work it does.
pub(crate) fn engine(name: &str) -> Option<(Engine, GpuEngine)> {
    let lower = name.to_ascii_lowercase();
    let after = |tag: &str| -> Option<&str> {
        let at = lower.find(tag)? + tag.len();
        Some(&lower[at..])
    };
    let num = |tag: &str| -> Option<u32> {
        let s = after(tag)?;
        let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
        s[..end].parse().ok()
    };
    let e = Engine {
        luid: luid(name)?,
        phys: num("_phys_")?,
        eng: num("_eng_")?,
    };
    Some((e, kind(after("_engtype_").unwrap_or(""))))
}

/// The kind of work of an engine type as the driver names it. Drivers differ:
/// "3D", "Compute_0", "Compute 1", "Cuda", "Copy", "VideoDecode",
/// "Video Decode 1", "VideoEncode", "Video Codec 0" (AMD's combined engine),
/// "VideoProcessing", "Video JPEG 0", "High Priority 3D", "Timer 0",
/// "Security 1", "True Audio 0" and more. Case, spaces, underscores, digits and
/// a "High Priority" in front do not change the kind.
pub(crate) fn kind(engtype: &str) -> GpuEngine {
    let key: String = engtype
        .chars()
        .filter(char::is_ascii_alphabetic)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let key = key.strip_prefix("highpriority").unwrap_or(&key);
    match key {
        "d" => GpuEngine::ThreeD, // "3D" without the digit
        "compute" | "cuda" => GpuEngine::Compute,
        "copy" => GpuEngine::Copy,
        "videodecode" => GpuEngine::VideoDecode,
        "videoencode" => GpuEngine::VideoEncode,
        k if k.starts_with("video") => GpuEngine::Video,
        _ => GpuEngine::Other,
    }
}

/// The busiest engine of each card, as Task Manager's GPU page shows it:
/// utilization is summed over the processes of each (adapter, physical GPU,
/// engine) and the largest of those sums is the card's figure, with the kind
/// of that engine. A tie goes to the 3D engine, then to the lowest engine
/// index, so an idle card leads with 3D. Instances with no valid reading or
/// no parsable name are skipped; a card with no valid instance is absent.
pub(crate) fn busiest_by_card<'a, I>(instances: I) -> HashMap<Luid, (f64, GpuEngine)>
where
    I: IntoIterator<Item = (&'a str, f64, bool)>,
{
    let mut sums: HashMap<Engine, (f64, GpuEngine)> = HashMap::new();
    for (name, value, valid) in instances {
        if !valid || !value.is_finite() {
            continue;
        }
        let Some((e, k)) = engine(name) else { continue };
        sums.entry(e).or_insert((0.0, k)).0 += value.max(0.0);
    }
    let mut engines: Vec<_> = sums.into_iter().collect();
    engines.sort_by_key(|(e, _)| *e);
    let mut out: HashMap<Luid, (f64, GpuEngine)> = HashMap::new();
    for (e, (v, k)) in engines {
        let v = v.clamp(0.0, 100.0);
        let best = out.entry(e.luid).or_insert((v, k));
        let tie_to_3d = v == best.0 && k == GpuEngine::ThreeD && best.1 != GpuEngine::ThreeD;
        if v > best.0 || tie_to_3d {
            *best = (v, k);
        }
    }
    out
}

/// Dedicated memory in use per card from `\GPU Adapter Memory(*)\Dedicated
/// Usage` (instances `luid_..._phys_0`), summed over the physical GPUs.
pub(crate) fn memory_by_card<'a, I>(instances: I) -> HashMap<Luid, u64>
where
    I: IntoIterator<Item = (&'a str, f64, bool)>,
{
    let mut out = HashMap::new();
    for (name, value, valid) in instances {
        if !valid || !value.is_finite() {
            continue;
        }
        if let Some(l) = luid(name) {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let bytes = value.max(0.0) as u64;
            *out.entry(l).or_insert(0) += bytes;
        }
    }
    out
}

/// A hardware card as the adapter list gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Adapter {
    pub luid: Luid,
    pub name: String,
    /// Dedicated video memory, bytes.
    pub total: u64,
}

/// The cards of one reading, in the order of `adapters` (the first is the one
/// Windows lists first). `engines` is `None` when the engine counter could not
/// be read: then no card has a busy figure. A card the engine counter has no
/// instance of is idle (no process uses it), so it reads 0, not "unknown".
/// Without an adapter list (`adapters` empty) the cards are the LUIDs the
/// counters name, without a name or a total.
pub(crate) fn cards(
    adapters: &[Adapter],
    engines: Option<&HashMap<Luid, (f64, GpuEngine)>>,
    memory: Option<&HashMap<Luid, u64>>,
) -> Vec<Gpu> {
    let make = |luid: Luid, name: &str, total: Option<u64>| {
        let (busy, lead) = match engines {
            Some(m) => {
                let (v, k) = m.get(&luid).copied().unwrap_or((0.0, GpuEngine::ThreeD));
                #[allow(clippy::cast_possible_truncation)]
                (Some(v as f32), Some(k))
            }
            None => (None, None),
        };
        Gpu {
            luid,
            name: name.to_owned(),
            busy_pct: busy,
            lead,
            mem_used: memory.and_then(|m| m.get(&luid).copied()),
            mem_total: total,
        }
    };
    if !adapters.is_empty() {
        return adapters
            .iter()
            .map(|a| make(a.luid, &a.name, (a.total > 0).then_some(a.total)))
            .collect();
    }
    let mut seen: Vec<Luid> = engines
        .map(|m| m.keys().copied().collect())
        .unwrap_or_default();
    seen.sort_unstable();
    seen.into_iter().map(|l| make(l, "", None)).collect()
}

/// The LUIDs the engine counter names that are neither a listed card nor a
/// known software adapter: a sign the adapter list is out of date (a card was
/// added, or a driver restart gave a card a new LUID).
pub(crate) fn unknown_luids<'a>(
    engines: &'a HashMap<Luid, (f64, GpuEngine)>,
    adapters: &[Adapter],
    software: &[Luid],
) -> impl Iterator<Item = Luid> + 'a {
    let known: Vec<Luid> = adapters
        .iter()
        .map(|a| a.luid)
        .chain(software.iter().copied())
        .collect();
    engines.keys().copied().filter(move |l| !known.contains(l))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CARD: Luid = 0x0001_8B3A;
    const A: &str = "pid_100_luid_0x00000000_0x00018B3A_phys_0_eng_0_engtype_3D";

    fn inst(pid: u32, luid: &str, eng: u32, engtype: &str) -> String {
        format!("pid_{pid}_luid_0x00000000_{luid}_phys_0_eng_{eng}_engtype_{engtype}")
    }

    #[test]
    fn parses_the_engine_from_an_instance_name() {
        let (e, k) = engine(A).unwrap();
        assert_eq!((e.luid, e.phys, e.eng, k), (CARD, 0, 0, GpuEngine::ThreeD));
        let (e2, _) = engine("pid_7_luid_0x00000001_0x0000D4A5_phys_1_eng_12_engtype_3D").unwrap();
        assert_eq!((e2.luid, e2.phys, e2.eng), (0x1_0000_D4A5, 1, 12));
        assert_eq!(engine("garbage"), None);
        assert_eq!(engine("pid_1_luid_0x1_phys_0_eng_0"), None);
        assert_eq!(engine("pid_1_luid_0xZZ_0x1_phys_0_eng_0_engtype_3D"), None);
        // Lower case, as PowerShell's Get-Counter shows them, parses the same.
        assert_eq!(engine(&A.to_ascii_lowercase()), engine(A));
    }

    #[test]
    fn every_engine_type_has_its_kind() {
        use GpuEngine::*;
        let cases = [
            // Seen on this PC (AMD Radeon RX 7800 XT and the Ryzen's Radeon
            // Graphics, Windows 11, 2026-11-27).
            ("3D", ThreeD),
            ("High Priority 3D", ThreeD),
            ("Compute 0", Compute),
            ("Compute 1", Compute),
            ("High Priority Compute", Compute),
            ("Copy", Copy),
            ("Video Codec 0", Video),
            ("Video Codec 1", Video),
            ("Video Decode 1", VideoDecode),
            ("Video JPEG 0", Video),
            ("Timer 0", Other),
            ("Security 1", Other),
            ("True Audio 0", Other),
            // Other drivers (NVIDIA, Intel, Microsoft Basic Render Driver).
            ("Compute_0", Compute),
            ("Compute_1", Compute),
            ("Cuda", Compute),
            ("VideoDecode", VideoDecode),
            ("VideoEncode", VideoEncode),
            ("Video Encode", VideoEncode),
            ("VideoProcessing", Video),
            ("LegacyOverlay", Other),
            ("GDI Render", Other),
            ("Crypto", Other),
            ("OFA", Other),
            ("", Other),
        ];
        for (engtype, want) in cases {
            assert_eq!(kind(engtype), want, "{engtype}");
            let name = inst(5, "0x00018B3A", 2, engtype);
            assert_eq!(engine(&name).map(|(_, k)| k), Some(want), "{name}");
        }
    }

    #[test]
    fn luids_parse_and_match_the_dxgi_parts() {
        assert_eq!(luid("luid_0x00000000_0x00018b3a_phys_0"), Some(CARD));
        assert_eq!(luid(A), Some(CARD));
        assert_eq!(luid_from_parts(0x0001_8B3A, 0), CARD);
        assert_eq!(luid_from_parts(5, -1), 0xFFFF_FFFF_0000_0005);
        assert_eq!(
            luid("luid_0xFFFFFFFF_0x00000005_phys_0"),
            Some(0xFFFF_FFFF_0000_0005)
        );
        assert_eq!(luid("no luid here"), None);
        assert_eq!(luid("luid_0x1"), None);
    }

    #[test]
    fn processes_on_one_engine_add_up() {
        let a2 = inst(200, "0x00018B3A", 0, "3D");
        let m = busiest_by_card([(A, 30.0, true), (a2.as_str(), 25.0, true)]);
        let (v, k) = m[&CARD];
        assert!((v - 55.0).abs() < 1e-9);
        assert_eq!(k, GpuEngine::ThreeD);
    }

    #[test]
    fn the_busiest_engine_of_any_kind_leads_not_only_3d() {
        // This PC at idle: a video engine busier than 3D, which the old
        // 3D-only reading showed as 1%.
        let codec = inst(100, "0x00018B3A", 12, "Video Codec 0");
        let compute = inst(300, "0x00018B3A", 2, "Compute 0");
        let m = busiest_by_card([
            (A, 0.8, true),
            (codec.as_str(), 9.3, true),
            (compute.as_str(), 2.0, true),
        ]);
        assert_eq!(m[&CARD].1, GpuEngine::Video);
        assert!((m[&CARD].0 - 9.3).abs() < 1e-9);
        // A model running on compute engines.
        let m = busiest_by_card([(A, 3.0, true), (compute.as_str(), 97.0, true)]);
        assert_eq!(m[&CARD], (97.0, GpuEngine::Compute));
    }

    #[test]
    fn different_engines_and_cards_take_the_max_not_the_sum() {
        let eng1 = inst(100, "0x00018B3A", 1, "3D");
        let other = inst(100, "0x0000FFFF", 0, "3D");
        let m = busiest_by_card([
            (A, 40.0, true),
            (eng1.as_str(), 35.0, true),
            (other.as_str(), 20.0, true),
        ]);
        assert!((m[&CARD].0 - 40.0).abs() < 1e-9);
        assert!((m[&0xFFFF].0 - 20.0).abs() < 1e-9);
    }

    #[test]
    fn an_idle_card_leads_with_3d_and_ties_prefer_3d() {
        let copy = inst(1, "0x00018B3A", 1, "Copy");
        let compute = inst(1, "0x00018B3A", 3, "Compute 1");
        let m = busiest_by_card([(copy.as_str(), 0.0, true), (A, 0.0, true)]);
        assert_eq!(m[&CARD], (0.0, GpuEngine::ThreeD));
        let m = busiest_by_card([(copy.as_str(), 5.0, true), (A, 5.0, true)]);
        assert_eq!(m[&CARD], (5.0, GpuEngine::ThreeD));
        // No 3D among the tied: the lowest engine index.
        let m = busiest_by_card([(compute.as_str(), 5.0, true), (copy.as_str(), 5.0, true)]);
        assert_eq!(m[&CARD], (5.0, GpuEngine::Copy));
    }

    #[test]
    fn invalid_instances_are_skipped_and_the_sum_is_capped() {
        let b = inst(2, "0x00018B3A", 0, "3D");
        let m = busiest_by_card([(A, 70.0, true), (b.as_str(), 70.0, true)]);
        assert_eq!(m[&CARD].0, 100.0);
        let m = busiest_by_card([(A, 10.0, true), (b.as_str(), 99.0, false)]);
        assert_eq!(m[&CARD].0, 10.0);
        let m = busiest_by_card([(A, f64::NAN, true), ("garbage", 50.0, true)]);
        assert!(m.is_empty());
        assert!(busiest_by_card(std::iter::empty()).is_empty());
    }

    #[test]
    fn memory_is_matched_to_the_card_by_luid() {
        let m = memory_by_card([
            ("luid_0x00000000_0x00018b3a_phys_0", 2.5e9, true),
            ("luid_0x00000000_0x00018b3a_phys_1", 1.0e9, true),
            ("luid_0x00000000_0x0001b735_phys_0", 0.0, true),
            ("luid_0x00000000_0x0001b6f8_phys_0", 7.0, false),
            ("_total", 9.0, true),
        ]);
        assert_eq!(m[&CARD], 3_500_000_000);
        assert_eq!(m[&0x0001_B735], 0);
        assert!(!m.contains_key(&0x0001_B6F8));
        let adapters = [
            Adapter {
                luid: 0x0001_B735,
                name: "AMD Radeon(TM) Graphics".into(),
                total: 512 << 20,
            },
            Adapter {
                luid: CARD,
                name: "AMD Radeon RX 7800 XT".into(),
                total: 16_368 << 20,
            },
        ];
        let engines = busiest_by_card([(A, 12.0, true)]);
        let g = cards(&adapters, Some(&engines), Some(&m));
        assert_eq!(g.len(), 2);
        assert_eq!(g[1].name, "AMD Radeon RX 7800 XT");
        assert_eq!(g[1].mem_used, Some(3_500_000_000));
        assert_eq!(g[1].mem_total, Some(16_368 << 20));
        assert_eq!(g[1].busy_pct, Some(12.0));
        // The idle card has no engine instance: 0, not unknown.
        assert_eq!(
            (g[0].busy_pct, g[0].lead),
            (Some(0.0), Some(GpuEngine::ThreeD))
        );
        assert_eq!(g[0].mem_used, Some(0));
    }

    #[test]
    fn no_engine_reading_is_none_not_zero() {
        let adapters = [Adapter {
            luid: CARD,
            name: "Card".into(),
            total: 0,
        }];
        let g = cards(&adapters, None, None);
        assert_eq!((g[0].busy_pct, g[0].lead), (None, None));
        assert_eq!((g[0].mem_used, g[0].mem_total), (None, None));
        assert!(cards(&[], None, None).is_empty());
    }

    #[test]
    fn without_an_adapter_list_the_counters_name_the_cards() {
        let other = inst(1, "0x0000FFFF", 0, "3D");
        let engines = busiest_by_card([(A, 5.0, true), (other.as_str(), 1.0, true)]);
        let g = cards(&[], Some(&engines), None);
        assert_eq!(g.iter().map(|c| c.luid).collect::<Vec<_>>(), [0xFFFF, CARD]);
        assert!(g.iter().all(|c| c.name.is_empty() && c.mem_total.is_none()));
    }

    #[test]
    fn a_luid_outside_the_list_is_noticed() {
        let soft = inst(1, "0x0001B6F8", 0, "3D");
        let new = inst(1, "0x00020000", 0, "3D");
        let engines = busiest_by_card([
            (A, 1.0, true),
            (soft.as_str(), 0.0, true),
            (new.as_str(), 0.0, true),
        ]);
        let adapters = [Adapter {
            luid: CARD,
            name: String::new(),
            total: 1,
        }];
        let unknown: Vec<_> = unknown_luids(&engines, &adapters, &[0x0001_B6F8]).collect();
        assert_eq!(unknown, [0x0002_0000]);
    }
}
