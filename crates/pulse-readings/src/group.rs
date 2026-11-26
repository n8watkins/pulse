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
    // Plain `vmmem` is the memory of Hyper-V, Windows Sandbox or Docker VMs.
    ("vmmem", "Virtual machine"),
];

/// Icon sources for rows that have no real file or whose own file has no
/// useful icon. Keys are lowercase image names without `.exe`; values are
/// files under `%SystemRoot%\System32` whose icon reads well at 24 px.
pub(crate) const STAND_INS: &[(&str, &str)] = &[
    // The penguin: only the WSL 2 VM.
    ("vmmemwsl", "wsl.exe"),
    // Other virtual machines: a neutral icon, not the penguin.
    ("vmmem", "taskmgr.exe"),
    // The Windows Security shield.
    ("msmpeng", "SecurityHealthSystray.exe"),
    // The services icon, for the shared service hosts.
    ("svchost", "services.exe"),
    // The memory rows: Task Manager's icon.
    ("memory compression", "taskmgr.exe"),
    ("system", "taskmgr.exe"),
    ("registry", "taskmgr.exe"),
    ("secure system", "taskmgr.exe"),
];

/// The stand-in file name for an image name, if it has one.
pub(crate) fn stand_in_file(image: &str) -> Option<&'static str> {
    let s = stem(image);
    STAND_INS
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(s))
        .map(|(_, f)| *f)
}

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

/// Whether the built-in map names this image (it then wins over the file's own description).
pub(crate) fn is_builtin(image: &str) -> bool {
    let s = stem(image);
    FRIENDLY.iter().any(|(k, _)| k.eq_ignore_ascii_case(s))
}

/// Pids kept per group, to try in turn when looking for one that can be opened.
const PIDS_KEPT: usize = 8;

/// Groups `(image name, private bytes, pid)` by image name (case-insensitive)
/// and returns the biggest `TOP` groups, largest first, each with the first
/// few pids seen in its group.
pub(crate) fn top_apps<I>(processes: I) -> Vec<(AppRow, Vec<u32>)>
where
    I: IntoIterator<Item = (String, u64, u32)>,
{
    let mut groups: HashMap<String, (AppRow, Vec<u32>)> = HashMap::new();
    for (image, bytes, pid) in processes {
        if image.is_empty() {
            continue;
        }
        let (row, pids) = groups.entry(image.to_ascii_lowercase()).or_insert_with(|| {
            let row = AppRow {
                name: friendly(&image),
                exe: image.clone(),
                processes: 0,
                mem_bytes: 0,
                cpu_pct: 0.0,
                path: None,
            };
            (row, Vec::new())
        });
        row.processes += 1;
        row.mem_bytes += bytes;
        if pids.len() < PIDS_KEPT {
            pids.push(pid);
        }
    }
    let mut rows: Vec<(AppRow, Vec<u32>)> = groups.into_values().collect();
    rows.sort_by(|a, b| {
        b.0.mem_bytes
            .cmp(&a.0.mem_bytes)
            .then_with(|| a.0.exe.cmp(&b.0.exe))
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
        assert_eq!(friendly("vmmem"), "Virtual machine");
        assert_eq!(friendly("chrome.exe"), "chrome");
        assert_eq!(friendly("Registry"), "Registry");
        assert!(is_builtin("SVCHOST.EXE") && !is_builtin("chrome.exe"));
    }

    #[test]
    fn only_vmmemwsl_gets_the_penguin() {
        assert_eq!(stand_in_file("vmmemWSL"), Some("wsl.exe"));
        assert_eq!(stand_in_file("vmmem"), Some("taskmgr.exe"));
        assert_eq!(stand_in_file("Vmmem.exe"), Some("taskmgr.exe"));
        assert_eq!(stand_in_file("chrome.exe"), None);
    }

    #[test]
    fn groups_sums_and_sorts() {
        let rows = top_apps([
            ("chrome.exe".to_string(), 100, 1),
            ("svchost.exe".to_string(), 30, 2),
            ("Chrome.exe".to_string(), 50, 3),
            ("svchost.exe".to_string(), 40, 4),
            (String::new(), 9999, 5),
        ]);
        let pids: Vec<Vec<u32>> = rows.iter().map(|r| r.1.clone()).collect();
        assert_eq!(pids, [vec![1, 3], vec![2, 4]]);
        let rows: Vec<AppRow> = rows.into_iter().map(|r| r.0).collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].exe, "chrome.exe");
        assert_eq!((rows[0].processes, rows[0].mem_bytes), (2, 150));
        assert_eq!(rows[1].name, "Windows services");
        assert_eq!((rows[1].processes, rows[1].mem_bytes), (2, 70));
    }

    #[test]
    fn keeps_only_the_top_rows() {
        let rows = top_apps((0..20u64).map(|i| (format!("a{i}.exe"), i, 0)));
        assert_eq!(rows.len(), TOP);
        assert_eq!(rows[0].0.mem_bytes, 19);
    }
}
