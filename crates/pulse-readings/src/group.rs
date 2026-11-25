//! Grouping of processes into app rows. Pure code, so it is tested everywhere.

use crate::AppRow;
use std::collections::HashMap;

/// How many rows the readers return.
pub(crate) const TOP: usize = 6;

/// Friendly names for images people would not recognise. Keys are lowercase
/// image names without `.exe`.
const FRIENDLY: &[(&str, &str)] = &[
    ("svchost", "Windows services"),
    ("msmpeng", "Microsoft Defender"),
    ("dwm", "Desktop Window Manager"),
    ("explorer", "Windows Explorer"),
    ("vmmemwsl", "WSL (Linux)"),
];

fn stem(image: &str) -> &str {
    let lower_ends_exe = image.len() > 4
        && image.is_char_boundary(image.len() - 4)
        && image[image.len() - 4..].eq_ignore_ascii_case(".exe");
    if lower_ends_exe {
        &image[..image.len() - 4]
    } else {
        image
    }
}

/// The name shown for an image such as `chrome.exe`.
pub(crate) fn friendly(image: &str) -> String {
    let s = stem(image);
    FRIENDLY
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(s))
        .map_or_else(|| s.to_string(), |(_, v)| (*v).to_string())
}

/// Groups `(image name, private bytes)` pairs by image name (case-insensitive)
/// and returns the biggest `TOP` groups, largest first.
pub(crate) fn top_apps<I>(processes: I) -> Vec<AppRow>
where
    I: IntoIterator<Item = (String, u64)>,
{
    let mut groups: HashMap<String, AppRow> = HashMap::new();
    for (image, bytes) in processes {
        if image.is_empty() {
            continue;
        }
        let row = groups
            .entry(image.to_ascii_lowercase())
            .or_insert_with(|| AppRow {
                name: friendly(&image),
                exe: image.clone(),
                processes: 0,
                mem_bytes: 0,
            });
        row.processes += 1;
        row.mem_bytes += bytes;
    }
    let mut rows: Vec<AppRow> = groups.into_values().collect();
    rows.sort_by(|a, b| {
        b.mem_bytes
            .cmp(&a.mem_bytes)
            .then_with(|| a.exe.cmp(&b.exe))
    });
    rows.truncate(TOP);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn friendly_names() {
        assert_eq!(friendly("svchost.exe"), "Windows services");
        assert_eq!(friendly("MsMpEng.exe"), "Microsoft Defender");
        assert_eq!(friendly("vmmemWSL"), "WSL (Linux)");
        assert_eq!(friendly("chrome.exe"), "chrome");
        assert_eq!(friendly("Registry"), "Registry");
    }

    #[test]
    fn groups_sums_and_sorts() {
        let rows = top_apps([
            ("chrome.exe".to_string(), 100),
            ("svchost.exe".to_string(), 30),
            ("Chrome.exe".to_string(), 50),
            ("svchost.exe".to_string(), 40),
            (String::new(), 9999),
        ]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].exe, "chrome.exe");
        assert_eq!((rows[0].processes, rows[0].mem_bytes), (2, 150));
        assert_eq!(rows[1].name, "Windows services");
        assert_eq!((rows[1].processes, rows[1].mem_bytes), (2, 70));
    }

    #[test]
    fn keeps_only_the_top_rows() {
        let rows = top_apps((0..20u64).map(|i| (format!("a{i}.exe"), i)));
        assert_eq!(rows.len(), TOP);
        assert_eq!(rows[0].mem_bytes, 19);
    }
}
