# HorseGirlRPC

A Hachimi plugin that shows live Umamusume career telemetry in Discord Rich Presence.

## Preview

**Da Wei**

![HorseGirlRPC Preview](preview.png)

**Also works with Alt Costume**

![HorseGirlRPC Alt Preview](preview-alt.png)

## Features

- Current trainee name and training scenario
- Live Speed, Stamina, Power, Guts, and Wit stats
- Dynamic large character icon for the active costume

## Compatibility & Testing

Only tested on the Global Steam Version, Idk if it works on other Version tho

It has **not been tested** with:

- Umamusume Pretty Derby – Japan (Steam)
- Umamusume Pretty Derby – Japan (DMM)
- Characters currently available only in the Japanese version and not yet released globally

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

## Credits

- [Umapyoi API](https://umapyoi.net/) — character and outfit metadata used to verify card/icon mappings.
- [Umamusume Wiki](https://umamusu.wiki/) — source of character icon images.
- [Hachimi](https://github.com/Hachimi-Hachimi/Hachimi) and [Hachimi Edge](https://github.com/kairusds/Hachimi-Edge) — plugin/mod loader and integration environment.

HorseGirlRPC is an independent fan project and is not affiliated with or endorsed by these projects or Cygames.