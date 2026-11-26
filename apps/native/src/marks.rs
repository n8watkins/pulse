//! Product marks for the Linux program rows.
//!
//! The marks are the SVG files of Simple Icons (`assets/marks/<slug>.svg`,
//! version 16.38.1, one 24 by 24 path each), embedded in the binary as text.
//! Only the path data is read, at the moment a row is built; nothing is parsed
//! or kept while the panel is hidden. A label without a mark keeps the
//! coloured letter tile.

/// One mark: its path data (24 by 24 view box), the brand colour of the tile
/// and the colour of the glyph on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mark {
    pub slug: &'static str,
    /// SVG path data of the glyph, 24 by 24.
    pub path: &'static str,
    /// Brand colour of the tile.
    pub rgb: [u8; 3],
    /// Black glyph (the brand colour is too light for a white one).
    pub dark_ink: bool,
    /// The tile is so dark that it needs a faint outline on a dark panel.
    pub ring: bool,
}

/// Slug, embedded file, brand colour (from the Simple Icons data).
type Entry = (&'static str, &'static str, [u8; 3]);

macro_rules! entry {
    ($slug:literal, $hex:literal) => {
        (
            $slug,
            include_str!(concat!("../assets/marks/", $slug, ".svg")),
            hex($hex),
        )
    };
}

#[allow(clippy::cast_possible_truncation)]
const fn hex(v: u32) -> [u8; 3] {
    [(v >> 16) as u8, (v >> 8) as u8, v as u8]
}

const ENTRIES: &[Entry] = &[
    entry!("claude", 0x00D9_7757),
    entry!("googlegemini", 0x008E_75B2),
    entry!("cursor", 0x0000_0000),
    entry!("ollama", 0x0000_0000),
    entry!("docker", 0x0024_96ED),
    entry!("nextdotjs", 0x0000_0000),
    entry!("nodedotjs", 0x005F_A04E),
    entry!("npm", 0x00CB_3837),
    entry!("pnpm", 0x00F6_9220),
    entry!("bun", 0x0000_0000),
    entry!("deno", 0x0000_0000),
    entry!("python", 0x0037_76AB),
    entry!("git", 0x00F0_3C2E),
    entry!("googlechrome", 0x0042_85F4),
    entry!("go", 0x0000_ADD8),
    entry!("postgresql", 0x0041_69E1),
    entry!("redis", 0x00FF_4438),
    entry!("vite", 0x0091_35FF),
    entry!("typescript", 0x0031_78C6),
    entry!("esbuild", 0x00FF_CF00),
    entry!("mongodb", 0x0047_A248),
    entry!("mysql", 0x0044_79A1),
    entry!("nginx", 0x0000_9639),
    entry!("tailscale", 0x0024_2424),
];

/// The `d` attribute of the one `<path>` of a Simple Icons file.
fn path_data(svg: &str) -> Option<&str> {
    let at = svg.find("<path d=\"")? + "<path d=\"".len();
    let len = svg[at..].find('"')?;
    Some(&svg[at..at + len])
}

/// Relative luminance of a colour (WCAG), 0 to 1.
fn luminance(rgb: [u8; 3]) -> f64 {
    let lin = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(rgb[0]) + 0.7152 * lin(rgb[1]) + 0.0722 * lin(rgb[2])
}

/// Contrast ratio of a white glyph on the colour.
fn white_contrast(rgb: [u8; 3]) -> f64 {
    1.05 / (luminance(rgb) + 0.05)
}

/// White is the brand's usual glyph colour; black only where white would be
/// under 3:1 (the WCAG minimum for graphics).
const MIN_WHITE_CONTRAST: f64 = 3.0;

/// Luminance under which a tile blends into the dark panel.
const RING_BELOW: f64 = 0.05;

/// The mark with this slug.
pub fn by_slug(slug: &str) -> Option<Mark> {
    let (slug, svg, rgb) = *ENTRIES.iter().find(|(s, _, _)| *s == slug)?;
    Some(Mark {
        slug,
        path: path_data(svg)?,
        rgb,
        dark_ink: white_contrast(rgb) < MIN_WHITE_CONTRAST,
        ring: luminance(rgb) < RING_BELOW,
    })
}

/// The first word of a label, lowercase, without a trailing colon.
fn first_word(label: &str) -> String {
    label
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_end_matches(':')
        .to_ascii_lowercase()
}

/// The slug of the mark for a Linux program label (as `pulse-readings`
/// writes it: "Claude Code", "node: next-server", "next-server (v16.4.6)",
/// "chrome-headless-shell", "npm exec next"), or `None`.
///
/// Programs that Simple Icons has no mark for (Codex CLI, the editor
/// servers) have none here, so they keep the letter.
pub fn slug_for(label: &str) -> Option<&'static str> {
    let lower = label.to_ascii_lowercase();
    let word = first_word(label);
    // Whatever runs under Node: the script says more than the runtime.
    let script = lower.strip_prefix("node: ").unwrap_or(&lower);
    if script.starts_with("next-") || script.starts_with("next ") || script.contains("turbopack") {
        return Some("nextdotjs");
    }
    let slug = match lower.as_str() {
        "claude code" => "claude",
        "gemini cli" => "googlegemini",
        "cursor agent" => "cursor",
        _ => match word.as_str() {
            "docker" | "dockerd" | "containerd" => "docker",
            "ollama" => "ollama",
            "node" | "nodejs" => "nodedotjs",
            "npm" | "npx" => "npm",
            "pnpm" | "pnpx" => "pnpm",
            "bun" => "bun",
            "deno" => "deno",
            "git" => "git",
            "go" | "gopls" => "go",
            "chrome" | "google-chrome" | "chrome-headless-shell" => "googlechrome",
            "redis" | "redis-server" => "redis",
            "postgres" | "postgresql" => "postgresql",
            "mysql" | "mysqld" => "mysql",
            "mongod" | "mongos" => "mongodb",
            "nginx" => "nginx",
            "tailscaled" | "tailscale" => "tailscale",
            "vite" => "vite",
            "esbuild" => "esbuild",
            "tsc" | "tsserver" => "typescript",
            w if w == "python" || w.starts_with("python3") || w.starts_with("python2") => "python",
            w if w.starts_with("git-") => "git",
            _ => return None,
        },
    };
    ENTRIES
        .iter()
        .find(|(s, _, _)| *s == slug)
        .map(|(s, _, _)| *s)
}

/// The mark for a label, when there is one.
pub fn for_label(label: &str) -> Option<Mark> {
    by_slug(slug_for(label)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_embedded_file_has_one_path() {
        for (slug, svg, _) in ENTRIES {
            let d = path_data(svg).unwrap_or_else(|| panic!("{slug}: no path"));
            assert!(d.len() > 20, "{slug}");
            assert!(d.starts_with(['M', 'm']), "{slug}");
            assert_eq!(svg.matches("<path").count(), 1, "{slug}");
            assert!(svg.contains("viewBox=\"0 0 24 24\""), "{slug}");
        }
        // No two entries share a slug.
        for (i, (a, _, _)) in ENTRIES.iter().enumerate() {
            assert!(ENTRIES[i + 1..].iter().all(|(b, _, _)| a != b), "{a}");
        }
    }

    #[test]
    fn labels_find_their_marks() {
        let want = |label: &str, slug: &str| {
            assert_eq!(slug_for(label), Some(slug), "{label}");
            assert_eq!(for_label(label).map(|m| m.slug), Some(slug), "{label}");
        };
        want("Claude Code", "claude");
        want("Gemini CLI", "googlegemini");
        want("Cursor Agent", "cursor");
        want("Docker", "docker");
        want("Ollama", "ollama");
        want("next-server (v16.4.6)", "nextdotjs");
        want("node: next-server", "nextdotjs");
        want("node: next-render-worker-app.js", "nextdotjs");
        want("node: pool_entry-[turbopack-node]_transforms", "nextdotjs");
        want("npm exec next start", "npm");
        want("node: shots.mjs", "nodedotjs");
        want("node", "nodedotjs");
        want("pnpm", "pnpm");
        want("pnpm exec vite", "pnpm");
        want("chrome-headless-shell", "googlechrome");
        want("bun: run", "bun");
        want("deno: main.ts", "deno");
        want("python3: server.py", "python");
        want("python3.12", "python");
        want("python", "python");
        want("git", "git");
        want("git-remote-https", "git");
        want("go", "go");
        want("gopls", "go");
        want("postgres", "postgresql");
        want("redis-server", "redis");
        want("tailscaled", "tailscale");
    }

    #[test]
    fn labels_without_a_mark_keep_the_letter() {
        // Simple Icons has no OpenAI or Visual Studio Code mark, and its Rust
        // logo is share-alike (CC BY-SA 4.0), which is not embedded.
        for l in [
            "cargo",
            "cargo-clippy",
            "rustc",
            "rust-analyzer",
            "Codex CLI",
            "Editor server",
            "Aider",
            "opencode",
            "tmux",
            "zsh",
            "systemd",
            "sccache",
            "java: app.jar",
            "ruby: x.rb",
            "",
        ] {
            assert_eq!(slug_for(l), None, "{l}");
            assert!(for_label(l).is_none(), "{l}");
        }
        // A word that only starts like a known program is not it.
        for l in ["nodemon", "gopher", "gitea", "bunny", "cargoship", "npmx"] {
            assert_eq!(slug_for(l), None, "{l}");
        }
    }

    #[test]
    fn ink_follows_contrast() {
        let m = |s| by_slug(s).unwrap();
        assert!(!m("nextdotjs").dark_ink && m("nextdotjs").ring);
        assert!(!m("docker").dark_ink && !m("googlechrome").dark_ink);
        assert!(!m("claude").dark_ink);
        // Light brands take a black glyph.
        assert!(m("esbuild").dark_ink && m("go").dark_ink && m("pnpm").dark_ink);
        assert!(!m("redis").ring && !m("docker").ring);
        // Every glyph is at least 3:1 on its tile.
        for (slug, _, rgb) in ENTRIES {
            let l = luminance(*rgb);
            let ratio = if by_slug(slug).unwrap().dark_ink {
                (l + 0.05) / 0.05
            } else {
                white_contrast(*rgb)
            };
            assert!(ratio >= 3.0, "{slug}: {ratio}");
        }
    }
}
