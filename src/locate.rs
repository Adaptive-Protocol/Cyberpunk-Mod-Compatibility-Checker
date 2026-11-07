//! Finds the game folder (Steam, GOG, Epic; Windows and Linux) and Mod Organizer 2's enabled mods. Read-only.
use std::{env, fs, iter, path::{Path, PathBuf}};

pub fn is_game(p: &Path) -> bool { p.join("bin/x64/Cyberpunk2077.exe").is_file() }

fn home() -> PathBuf { env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_default() }

/// `"key"  "value"` pairs from a Steam .vdf or a JSON-ish manifest line, with `\\` unescaped.
fn values<'a>(text: &'a str, key: &'a str) -> impl Iterator<Item = String> + 'a {
    let key = format!("\"{key}\"");
    text.lines().filter_map(move |l| {
        let rest = l.trim().strip_prefix(&key)?.trim_start_matches([' ', '\t', ':']).trim().trim_end_matches(',');
        Some(rest.strip_prefix('"')?.strip_suffix('"')?.replace("\\\\", "\\"))
    })
}

fn steam_roots() -> Vec<PathBuf> {
    if cfg!(windows) { return vec!["C:/Program Files (x86)/Steam".into(), "C:/Program Files/Steam".into()] }
    [".steam/steam", ".local/share/Steam", ".var/app/com.valvesoftware.Steam/.local/share/Steam"].iter().map(|p| home().join(p)).collect()
}

/// The first Cyberpunk 2077 install found, or None (the user picks it then).
pub fn find_game() -> Option<PathBuf> {
    let h = home();
    let mut c: Vec<PathBuf> = steam_roots().iter().flat_map(|s| {
        let libs = fs::read_to_string(s.join("steamapps/libraryfolders.vdf")).map(|t| values(&t, "path").map(PathBuf::from).collect::<Vec<_>>()).unwrap_or_default();
        iter::once(s.clone()).chain(libs).map(|l| l.join("steamapps/common/Cyberpunk 2077"))
    }).collect();
    if cfg!(windows) {
        let epic = fs::read_dir("C:/ProgramData/Epic/EpicGamesLauncher/Data/Manifests").into_iter().flatten().flatten()
            .filter_map(|e| fs::read_to_string(e.path()).ok()).filter(|t| t.contains("\"Cyberpunk 2077\""))
            .flat_map(|t| values(&t, "InstallLocation").map(PathBuf::from).collect::<Vec<_>>());
        c.extend(epic);
        c.extend(["C:/GOG Games/Cyberpunk 2077", "C:/Program Files (x86)/GOG Galaxy/Games/Cyberpunk 2077", "C:/Program Files/Epic Games/Cyberpunk2077"].map(PathBuf::from));
    } else {
        c.extend(["Games/Heroic/Cyberpunk 2077", "Games/Heroic/Cyberpunk2077"].map(|p| h.join(p)));
    }
    c.into_iter().find(|p| is_game(p))
}

/// DLLs set to load from the game folder ("native") in a Wine prefix's user.reg.
fn reg_overrides(reg: &str) -> Vec<String> {
    let mut on = false;
    reg.lines().filter_map(|l| {
        if l.starts_with('[') { on = l.starts_with("[Software\\\\Wine\\\\DllOverrides]"); return None }
        let (k, v) = l.split_once('=').filter(|_| on)?;
        v.trim().trim_matches('"').starts_with('n').then(|| k.trim().trim_matches('"').trim_start_matches('*').to_lowercase())
    }).collect()
}

/// DLLs set to native by WINEDLLOVERRIDES in Cyberpunk's (Steam app 1091500) launch options in a localconfig.vdf,
/// e.g. `WINEDLLOVERRIDES="winmm,version=n,b" %command%`.
fn launch_overrides(vdf: &str) -> Vec<String> {
    let mut depth: Option<i32> = None;                         // inside the app's block, how deep
    for l in vdf.lines().map(str::trim) {
        match depth {
            None if l == "\"1091500\"" => depth = Some(0),
            Some(d) if l == "{" => depth = Some(d + 1),
            Some(d) if l == "}" => depth = (d > 1).then_some(d - 1),
            Some(1) if l.starts_with("\"LaunchOptions\"") => {
                let opts = l["\"LaunchOptions\"".len()..].trim().trim_matches('"').replace("\\\"", "\"");
                let Some(v) = opts.split("WINEDLLOVERRIDES=").nth(1) else { return vec![] };
                let v = v.strip_prefix('"').map_or_else(|| v.split_whitespace().next().unwrap_or(""), |q| q.split('"').next().unwrap_or(""));
                return v.split(';').filter_map(|e| e.split_once('=')).filter(|(_, mode)| mode.trim().starts_with('n'))
                    .flat_map(|(dlls, _)| dlls.split(',').map(|d| d.trim().to_lowercase())).collect();
            }
            _ => {}
        }
    }
    vec![]
}

/// On Linux under Steam's Proton, the DLLs Wine loads from the game folder instead of its own: from the game's
/// prefix and its launch options. None when that can't be told (Windows, or no Proton prefix next to the game).
pub fn wine_overrides(game: &Path) -> Option<Vec<String>> {
    if cfg!(windows) { return None }
    let steamapps = game.parent()?.parent()?;                  // <library>/steamapps/common/<game folder>
    let mut v = reg_overrides(&fs::read_to_string(steamapps.join("compatdata/1091500/pfx/user.reg")).ok()?);
    for user in steam_roots().iter().flat_map(|r| fs::read_dir(r.join("userdata")).into_iter().flatten().flatten()) {
        if let Ok(t) = fs::read_to_string(user.path().join("config/localconfig.vdf")) { v.extend(launch_overrides(&t)) }
    }
    Some(v)
}

/// Settings from a ModOrganizer.ini, with Qt's `@ByteArray(...)` wrapper and doubled backslashes removed.
fn ini(text: &str, key: &str) -> Option<String> {
    let v = text.lines().find_map(|l| l.trim().strip_prefix(key)?.strip_prefix('='))?.trim();
    let v = v.strip_prefix("@ByteArray(").and_then(|v| v.strip_suffix(')')).unwrap_or(v);
    Some(v.replace("\\\\", "\\")).filter(|v| !v.is_empty())
}

fn same_dir(a: &str, b: &Path) -> bool {
    let n = |s: &str| s.replace('\\', "/").trim_end_matches('/').to_lowercase();
    n(a) == n(&b.to_string_lossy())
}

/// A Mod Organizer 2 instance (its folder holds ModOrganizer.ini) set up for this game, on Windows.
pub fn find_mo2(game: &Path) -> Option<PathBuf> {
    let base = env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("ModOrganizer"))?;
    fs::read_dir(base).ok()?.flatten().map(|e| e.path()).find(|d| {
        fs::read_to_string(d.join("ModOrganizer.ini")).is_ok_and(|t| ini(&t, "gamePath").is_some_and(|g| same_dir(&g, game)))
    })
}

/// The enabled mod folders of an MO2 instance's selected profile, highest priority first. Each is laid over the game folder.
pub fn mo2_mods(instance: &Path) -> Result<Vec<PathBuf>, String> {
    let t = fs::read_to_string(instance.join("ModOrganizer.ini")).map_err(|_| format!("No ModOrganizer.ini in {}.", instance.display()))?;
    let base = ini(&t, "base_directory").map_or_else(|| instance.to_path_buf(), PathBuf::from);
    let dir = |key: &str, default: &str| ini(&t, key).map_or_else(|| base.join(default), |v| PathBuf::from(v.replace("%BASE_DIR%", &base.to_string_lossy())));
    let profile = ini(&t, "selected_profile").unwrap_or_else(|| "Default".into());
    let list = fs::read_to_string(dir("profiles_directory", "profiles").join(&profile).join("modlist.txt"))
        .map_err(|_| format!("Couldn't read the mod list of MO2 profile {profile:?}."))?;
    let mods = dir("mod_directory", "mods");
    let overwrite = dir("overwrite_directory", "overwrite");   // what the game wrote through MO2, such as logs; above every mod
    Ok(iter::once(overwrite).chain(list.lines().filter_map(|l| l.strip_prefix('+')).map(|n| mods.join(n.trim()))).filter(|p| p.is_dir()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steam_library_paths() {
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n";
        assert_eq!(values(vdf, "path").collect::<Vec<_>>(), ["C:\\Program Files (x86)\\Steam", "D:\\SteamLibrary"]);
        assert_eq!(values("  \"InstallLocation\": \"D:\\\\Epic\\\\Cyberpunk2077\",", "InstallLocation").next().unwrap(), "D:\\Epic\\Cyberpunk2077");
    }

    #[test]
    fn mo2_instance() {
        let d = env::temp_dir().join(format!("ccc-mo2-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        for m in ["A", "B", "C"] { fs::create_dir_all(d.join("mods").join(m)).unwrap() }
        fs::create_dir_all(d.join("profiles/Main")).unwrap();
        fs::write(d.join("ModOrganizer.ini"), "[General]\ngamePath=@ByteArray(D:\\\\Games\\\\Cyberpunk 2077)\nselected_profile=@ByteArray(Main)\n").unwrap();
        fs::write(d.join("profiles/Main/modlist.txt"), "# comment\n+B\n-C\n+A\n*DLC\n").unwrap();
        assert_eq!(ini(&fs::read_to_string(d.join("ModOrganizer.ini")).unwrap(), "gamePath").unwrap(), "D:\\Games\\Cyberpunk 2077");
        assert!(same_dir("D:\\Games\\Cyberpunk 2077\\", Path::new("d:/games/cyberpunk 2077")));
        assert_eq!(mo2_mods(&d).unwrap(), [d.join("mods/B"), d.join("mods/A")]);
        fs::create_dir_all(d.join("overwrite")).unwrap();
        assert_eq!(mo2_mods(&d).unwrap()[0], d.join("overwrite"));
    }

    #[test]
    fn wine_dll_overrides() {
        let reg = "WINE REGISTRY Version 2\n\n[Software\\\\Wine\\\\DllOverrides] 1730000000\n#time=1\n\"*d3dcompiler_47\"=\"native\"\n\"version\"=\"native,builtin\"\n\"winmm\"=\"builtin\"\n\n[Software\\\\Other] 1\n\"x\"=\"native\"\n";
        assert_eq!(reg_overrides(reg), ["d3dcompiler_47", "version"]);
        let vdf = "\"apps\"\n{\n\t\"1091500\"\n\t{\n\t\t\"cloud\"\n\t\t{\n\t\t\t\"LaunchOptions\"\t\t\"nested\"\n\t\t}\n\t\t\"LaunchOptions\"\t\t\"WINEDLLOVERRIDES=\\\"winmm,version=n,b\\\" %command%\"\n\t}\n}\n";
        assert_eq!(launch_overrides(vdf), ["winmm", "version"]);
        assert_eq!(launch_overrides("\"1091500\"\n{\n\"LaunchOptions\" \"WINEDLLOVERRIDES=winmm=n;version=b %command%\"\n}"), ["winmm"]);
        assert!(launch_overrides("\"1091501\"\n{\n\"LaunchOptions\" \"WINEDLLOVERRIDES=winmm=n\"\n}").is_empty());
    }
}
