# HorseGirlRPC

A Hachimi plugin that shows your live Umamusume career and current screen in Discord Rich Presence.

## Preview

**Da Wei**

![HorseGirlRPC Preview](preview.png)

**Also works with Alt Costume**

![HorseGirlRPC Alt Preview](preview-alt.png)

## Features

**Training (career)**

- Current Trainee name and training scenario
- Live Speed, Stamina, Power, Guts, and Wit stats
- Dynamic large Trainee icon for the active costume
- Energy and Mood on Trainee icon hover, e.g. `Energy 65/100 | Mood GOOD` (1.1.0)

**Outside career**

- Activity title is always `Umamusume`, so the game's own Rich Presence never takes over the slot (1.2.0)
- Current screen in the activity details: Home, Scouting, Reading Story, Club, Team Trials, Champions Meeting, Character Gallery, Events, Daily/Legend Race, Room Match, Practice Race, and more (1.2.0)
- Extra screen state when the game exposes it (1.2.0):
  - Story: chapter/episode, character story, or event story title
  - Events: event name, playback title, or event points
  - Champions Meeting: league and current round W-L
  - Team Trials: live rank and promotion points
  - Club: club name
  - Gallery: character and card being viewed
  - Extra Story: episode title
- Training stats only show during an active career and never go stale on other screens (1.2.0)

*Per-screen extra state is best-effort: if a screen shows only its name, the current game build doesn't expose that data (or it isn't mapped and founded yet).*

## Compatibility & Testing

Only tested on the **Global Steam** Version.
Idk if it works on **Japanese Steam/DMM** tho, also idk if it can display Trainee that's not released on Global yet

## Installation

1. Build or download from [Releases](https://github.com/BeLfayza/horse-girl-rpc/releases).
2. Copy it to the game's `hachimi` directory, for example:

   ```text
   UmamusumePrettyDerby/hachimi/horse_girl_rpc.dll
   ```

3. Add the DLL to `hachimi/config.json`:

   ```json
   {
     "load_libraries": [
       "hachimi\\horse_girl_rpc.dll"
     ]
   }
   ```

4. Restart the game with Discord running.

## Build

```powershell
cargo check
cargo build --release
```

## Credits

- [Umapyoi API](https://umapyoi.net/) — character and outfit metadata used to verify card/icon mappings.
- [Umamusume Wiki](https://umamusu.wiki/) — source of character icon images.
- [Hachimi](https://github.com/Hachimi-Hachimi/Hachimi) and [Hachimi Edge](https://github.com/kairusds/Hachimi-Edge) — plugin/mod loader and integration environment.

HorseGirlRPC is an independent fan project and is not affiliated with or endorsed by these projects or Cygames.