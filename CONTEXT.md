# Cyberpunk mod compatibility checker

Warns a player, before they launch the game, that an installed mod is likely to break on their game build.

Read-only unless the player asks: **Back up and remove** or **Delete** for a red mod, and **Restore** or delete for a **Backup**. Each shows every file first, and none touches the game's own files or a **Framework**. (The maintainer added these file actions on 2026-11-04, before the first release; until then the app was planned as read-only for good.)

App name: Cyberpunk Mod Compatibility Checker. Repo: Adaptive-Protocol/Cyberpunk-Mod-Compatibility-Checker. Own Nexus page, separate from the dedupe app.

## Language

**Game build**:
The Cyberpunk 2077 version installed on the player's machine (e.g. 2.5).
_Avoid_: patch, game version (when ambiguous with a mod's version)

**Framework**:
A core mod that hooks the game at a low level and must be updated after most game builds (RED4ext, CET, redscript, ArchiveXL, TweakXL, Codeware, ...). the maintainer curates the list.
_Avoid_: mod architecture, core mod (fine in prose, not as the term)

**Dependent mod**:
A mod that relies heavily on one or more Frameworks, or touches the engine directly (engine, audio, weather mods are typical), so it is a likely culprit after a new Game build.

**Compatibility manifest**:
the maintainer's curated file mapping Game builds to the Framework versions and Dependent mods known to work with them. Lives in the maintainer's GitHub repo; the utility only ever reads a local copy.
_Avoid_: update status, update manifest

**Last launch**:
The game's most recent run, as its logs record it: RED4ext's log, redscript's log, mods' own logs (ArchiveXL's names the world edits it skipped) and Advanced Crash Reporter's reports. Its findings update after each launch.

**Loader**:
A file in bin/x64 that loads a **Framework**: RED4ext's winmm.dll, or the ASI loader version.dll that loads CET and .asi plugins. Listed as the Framework's `parts`.

**Pattern**:
A known clash between files, kept in the **Compatibility manifest** and checked on every scan: red when all its paths are installed and none of its `none` paths are. Its optional `remove` names what the app may offer to remove.

**Mod entry**:
A whole folder or file directly inside a mod folder (archive/pc/mod, r6/scripts, r6/tweaks, red4ext/plugins, CET's mods, REDmod's mods, or an .asi in bin/x64/plugins). The only thing the app may move or delete.
_Avoid_: mod file (a mod usually has several entries)

**Leftover**:
A file the **Last launch** shows is in the way: a mod's own log calls it older, conflicting or still installed ("Conflicting older … component: <path>"), or RED4ext closed the game because the script declares native code no loaded plugin provides. Red. Remove offers it, with the same-named files beside it, only when it's a whole **Mod entry**; one inside another mod's folder is left to the player.

**Companion**:
A **Mod entry** in another mod folder with the same name as a red mod's own entry (e.g. r6/tweaks/X next to r6/scripts/X). It's offered with the mod and marked "same name".

**Add-on**:
A mod whose scripts build on a red mod: they import a module only that mod declares, or use a class or function it defines in a module they import or add to (EquipmentEx's add-ons add to its module). Without the red mod, redscript can't compile the add-on, which stops every script mod, so Remove offers the add-on and its **Companions** with the mod, marked "builds on X". Mods that build on an add-on count too. An import under `@if(ModuleExists(..))` is optional and doesn't count.
_Avoid_: dependency (a **Dependent mod** depends on a **Framework**)

**Back up and remove**:
Moving a red mod's ticked **Mod entries** into a **Backup** until the mod is updated.
_Avoid_: disable, uninstall, quarantine

**Backup**:
A folder `_mod_backups/<date> <mod>/` in the game folder that holds the moved **Mod entries** at their original paths, plus backup.toml (name, why, date, paths).

**Restore**:
Moving a **Backup**'s entries back into the game folder. It's refused if any of them has been installed again.

**Delete**:
Permanently removing ticked **Mod entries** after a second confirmation, or removing a **Backup**.

**Bridge agent**:
A developer-side Claude agent the maintainer runs after a new Game build. It researches Framework support and proposes an updated Compatibility manifest. It never ships to players.

## Relationships

- A **Compatibility manifest** has one entry per **Game build**.
- Each entry lists the minimum working version of each **Framework**, and the **Dependent mods** at risk with the **Frameworks** they need.
- Each **Framework** and **Dependent mod** carries its Nexus mod ID, which gives a fixed mod page link (nexusmods.com/cyberpunk2077/mods/<id>).
- The utility scans the player's game install folder (not a downloads folder).
- A mod page link opens in the player's own browser only when they click it; the utility never connects itself. Links are built from the Nexus mod ID, so they always point at nexusmods.com. A security audit runs after the build.
- Verdicts: red (likely to break), green (OK for this Game build), gray (not confirmed: mod not in manifest, or manifest older than the Game build). A banner warns when the Game build is newer than the manifest.
- A **Framework** can be marked build-locked (CET almost never survives a patch): on an unconfirmed newer Game build it shows red, not gray.
- A **Dependent mod** is recognised by a marker file or folder only (no version); its verdict comes from the **Frameworks** it needs, plus any known-broken note (e.g. "broken on 2.6, fixed in 1.3").
- Known-broken notes come from GitHub releases and from bug reports the maintainer collects from Nexus comments; each note records its source and date.
- The utility finds the game folder itself (Steam, GOG, Epic; Windows and Linux), with a folder picker as fallback. If it detects Mod Organizer 2, it also scans the MO2 mods folder (v1).
- Only **Mod entries** inside the game folder can be moved or deleted, never a **Framework**, its **Loader** or the game's own files. A path that leaves the game folder is refused, including one from a drop-in compat.toml.
- Mods installed with Mod Organizer 2 are never moved: the app says to untick them in MO2.
- **Delete** backs up first and then deletes the **Backup**, so a failed move changes nothing.
- The **Bridge agent** (.claude/agents/compat-bridge.md) writes the **Compatibility manifest**; the utility reads it, offline.

## Flagged ambiguities

- "update status" / "directed update status" resolved to **Compatibility manifest**.
- "read-only" (the 2026-10-22 plan) resolved to read-only unless the player asks for a file action (2026-11-04).
