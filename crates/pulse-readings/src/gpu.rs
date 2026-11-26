//! Graphics use from the per-process engine counters. Pure code, so it is tested everywhere.

use std::collections::HashMap;

/// The engine an instance belongs to: adapter LUID, physical GPU and engine index.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Engine {
    luid: String,
    phys: u32,
    eng: u32,
}

/// Parses `pid_123_luid_0x00000000_0x0000D4A5_phys_0_eng_3_engtype_3D`.
pub(crate) fn engine(name: &str) -> Option<Engine> {
    let lower = name.to_ascii_lowercase();
    let after = |tag: &str| -> Option<&str> {
        let at = lower.find(tag)? + tag.len();
        Some(&lower[at..])
    };
    // The LUID is two hex words joined by an underscore.
    let rest = after("luid_")?;
    let mut parts = rest.split('_');
    let (hi, lo) = (parts.next()?, parts.next()?);
    if !hi.starts_with("0x") || !lo.starts_with("0x") {
        return None;
    }
    let num = |tag: &str| -> Option<u32> {
        let s = after(tag)?;
        let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
        s[..end].parse().ok()
    };
    Some(Engine {
        luid: format!("{hi}_{lo}"),
        phys: num("phys_")?,
        eng: num("eng_")?,
    })
}

/// The busiest engine, as Task Manager shows: utilisation is summed over the
/// processes of each (adapter, physical GPU, engine) and the largest of those
/// sums is the figure. Instances with no valid reading are skipped; `None`
/// when there is no valid instance at all (never `Some(0.0)` for "unknown").
pub(crate) fn busiest<'a, I>(instances: I) -> Option<f64>
where
    I: IntoIterator<Item = (&'a str, f64, bool)>,
{
    let mut sums: HashMap<Option<Engine>, f64> = HashMap::new();
    for (name, value, valid) in instances {
        if !valid || !value.is_finite() {
            continue;
        }
        *sums.entry(engine(name)).or_insert(0.0) += value.max(0.0);
    }
    sums.into_values()
        .max_by(f64::total_cmp)
        .map(|v| v.clamp(0.0, 100.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "pid_100_luid_0x00000000_0x0000D4A5_phys_0_eng_0_engtype_3D";

    #[test]
    fn parses_the_engine_from_an_instance_name() {
        let e = engine(A).unwrap();
        assert_eq!((e.phys, e.eng), (0, 0));
        let e2 = engine("pid_7_luid_0x00000000_0x0000D4A5_phys_1_eng_12_engtype_3D").unwrap();
        assert_eq!((e2.phys, e2.eng), (1, 12));
        assert_eq!(engine("garbage"), None);
        assert_eq!(engine("pid_1_luid_0x1_phys_0_eng_0"), None);
    }

    #[test]
    fn processes_on_one_engine_add_up() {
        let a2 = "pid_200_luid_0x00000000_0x0000D4A5_phys_0_eng_0_engtype_3D";
        let v = busiest([(A, 30.0, true), (a2, 25.0, true)]).unwrap();
        assert!((v - 55.0).abs() < 1e-9);
    }

    #[test]
    fn different_engines_and_adapters_take_the_max_not_the_sum() {
        let eng1 = "pid_100_luid_0x00000000_0x0000D4A5_phys_0_eng_1_engtype_3D";
        let other = "pid_100_luid_0x00000000_0x0000FFFF_phys_0_eng_0_engtype_3D";
        let v = busiest([(A, 40.0, true), (eng1, 35.0, true), (other, 20.0, true)]).unwrap();
        assert!((v - 40.0).abs() < 1e-9);
    }

    #[test]
    fn nothing_valid_is_none_not_zero() {
        assert_eq!(busiest(std::iter::empty()), None);
        assert_eq!(busiest([(A, 12.0, false)]), None);
        assert_eq!(busiest([(A, 0.0, true)]), Some(0.0));
    }

    #[test]
    fn invalid_instances_are_skipped_and_the_sum_is_capped() {
        let b = "pid_2_luid_0x00000000_0x0000D4A5_phys_0_eng_0_engtype_3D";
        assert_eq!(busiest([(A, 70.0, true), (b, 70.0, true)]), Some(100.0));
        assert_eq!(busiest([(A, 10.0, true), (b, 99.0, false)]), Some(10.0));
    }
}
