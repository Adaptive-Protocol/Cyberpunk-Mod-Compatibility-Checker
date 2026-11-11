# What's safe on patch 2.5

Confirmed, 2026-11-04. The list for game build 2.5 passed a launch test: the game ran and closed normally with every listed framework loaded, so mods that match it show as OK.

**Frameworks in the list, with the lowest version known to work on 2.5:** RED4ext 1.31.0, Cyber Engine Tweaks 1.38.0, ArchiveXL 1.28.0, TweakXL 1.12.0, Codeware 1.21.2, Mod Settings 0.2.28, Input Loader 0.2.6. redscript, RedFileSystem, Native Settings UI and Audioware are checked for presence only.

**Known problems on 2.5:** mods made for an older patch that replace the game's sound index files (`sfx_container.opusinfo`, `eventsmetadata.json`) mute menu, footstep, tire and ambient sounds. The app flags them from v1.4.0 (source: the maintainer's 2.5 install, 2026-11-08). An older Night City Allies doesn't compile on 2.5, and redscript then runs no script mod at all; delete its folder and install the current version (source: the maintainer's 2.5 install, 2026-11-11).

Researched by a Claude agent and reviewed by the maintainer.
