# HorseGirlRPC

A Hachimi plugin that shows live Umamusume career telemetry in Discord Rich Presence. (Iya, ini hasil mecutin AI)

## Preview

**Da Wei**

![HorseGirlRPC Preview](preview.png)

**Also works with Alt Costume**

![HorseGirlRPC Alt Preview](preview-alt.png)

## Features

- Current trainee name and training scenario
- Live Speed, Stamina, Power, Guts, and Wit stats
- Dynamic large character icon for the active costume
- Costume-to-card detection with selected-card and base-card fallbacks
- Cached, asynchronous icon lookup that does not block the game thread

Character icon file URLs are resolved through the Umamusume Wiki MediaWiki API. If an icon or network request is unavailable, the text-only Rich Presence keeps working.

## Compatibility & Testing

HorseGirlRPC has only been tested with **Umamusume Pretty Derby – Global (Steam)**, using characters that have already been released in the Global version.

It has **not been tested** with:

- Umamusume Pretty Derby – Japan (Steam)
- Umamusume Pretty Derby – Japan (DMM)
- Characters currently available only in the Japanese version and not yet released globally

Compatibility with these versions and characters is not guaranteed.

## Installation

1. Build or download `horse_girl_rpc.dll`.
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

The DLL is created at `target/release/horse_girl_rpc.dll`.

## Credits

- [Umapyoi API](https://umapyoi.net/) — character and outfit metadata used to verify card/icon mappings.
- [Umamusume Wiki](https://umamusu.wiki/) — source of character icon images.
- [Hachimi](https://github.com/Hachimi-Hachimi/Hachimi) and [Hachimi Edge](https://github.com/kairusds/Hachimi-Edge) — plugin/mod loader and integration environment.

HorseGirlRPC is an independent fan project and is not affiliated with or endorsed by these projects or Cygames.