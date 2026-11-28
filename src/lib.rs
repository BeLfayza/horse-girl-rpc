mod hachimi;
mod icon;
pub(crate) mod telemetry;

use discord_rich_presence::{DiscordIpc, DiscordIpcClient, activity};
use serde::{Deserialize, Serialize};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hachimi::{HachimiApi, HachimiGetApiFn};

const DISCORD_APP_ID: &str = "1574266970763694212";

#[repr(i32)]
pub enum InitResult {
    Error = 0,
    Ok = 1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrainingStats {
    pub speed: u32,
    pub stamina: u32,
    pub power: u32,
    pub guts: u32,
    pub wit: u32,
}

impl TrainingStats {
    pub fn to_discord_state(&self) -> String {
        format!(
            "SPD {} · STA {} · POW {} · GUT {} · WIT {}",
            self.speed, self.stamina, self.power, self.guts, self.wit
        )
    }
}

enum RpcUpdate<'a> {
    Set(activity::Activity<'a>),
}

fn start_rpc_worker() -> Result<(), String> {
    thread::Builder::new()
        .name("HorseGirlRPC".into())
        .spawn(|| {
            loop {
                let mut client = DiscordIpcClient::new(DISCORD_APP_ID);

                if client.connect().is_ok() {
                    // Unix-ms marker for the elapsed timer; set when a training
                    // session is first observed, cleared when it ends.
                    let mut training_started_ms: Option<i64> = None;

                    loop {
                        if let telemetry::GameSnapshot::Training(_) = telemetry::snapshot() {
                            if training_started_ms.is_none() {
                                training_started_ms = Some(
                                    SystemTime::now()
                                        .duration_since(UNIX_EPOCH)
                                        .map(|since| since.as_millis() as i64)
                                        .unwrap_or(0),
                                );
                            }
                        } else {
                            training_started_ms = None;
                        }

                        let result = match telemetry::snapshot() {
                            telemetry::GameSnapshot::Training(telemetry) => {
                                let activity_name = telemetry.discord_activity_name();
                                let details = telemetry.discord_details();
                                let state = telemetry.discord_state();
                                let presence = activity::Activity::new()
                                    .name(activity_name)
                                    .activity_type(activity::ActivityType::Playing)
                                    .details(details)
                                    .state(state);
                                let presence = match telemetry.icon.and_then(icon::get_or_request)
                                {
                                    Some(icon) => presence.assets(
                                        activity::Assets::new()
                                            .large_image(icon.url)
                                            .large_text(telemetry.discord_large_text()),
                                    ),
                                    None => presence,
                                };
                                // A real elapsed clock: minutes since the training
                                // session began, so Discord renders a normal M:SS timer.
                                let presence = match training_started_ms {
                                    Some(started_ms) => {
                                        presence.timestamps(
                                            activity::Timestamps::new().start(started_ms),
                                        )
                                    }
                                    None => presence,
                                };

                                RpcUpdate::Set(presence)
                            }
                            telemetry::GameSnapshot::MainMenu {
                                title,
                                details,
                                state,
                            } => {
                                let mut activity = activity::Activity::new()
                                    .name(title)
                                    .activity_type(activity::ActivityType::Playing)
                                    .details(details);
                                if let Some(state) = state {
                                    activity = activity.state(state);
                                }
                                RpcUpdate::Set(activity)
                            }
                            telemetry::GameSnapshot::Unavailable => {
                                // Keep a placeholder activity so the game's own
                                // Discord Rich Presence never takes over the slot.
                                RpcUpdate::Set(
                                    activity::Activity::new()
                                        .name("Umamusume")
                                        .activity_type(activity::ActivityType::Playing),
                                )
                            }
                        };

                        let send_result = match result {
                            RpcUpdate::Set(presence) => client.set_activity(presence),
                        };

                        if send_result.is_err() {
                            let _ = client.close();
                            break;
                        }

                        thread::sleep(Duration::from_secs(5));
                    }
                }

                thread::sleep(Duration::from_secs(5));
            }
        })
        .map(|_| ())
        .map_err(|error| format!("failed to start Discord RPC worker: {error}"))
}

#[unsafe(no_mangle)]
pub extern "C" fn hachimi_init_v3(get_api: HachimiGetApiFn, version: i32) -> InitResult {
    if version < 3 {
        return InitResult::Error;
    }

    let api = match HachimiApi::resolve(get_api) {
        Ok(api) => api,
        Err(_) => return InitResult::Error,
    };
    if hachimi::install(api).is_err() {
        return InitResult::Error;
    }

    if let Err(error) = telemetry::initialize(&api) {
        hachimi::log_error(format!("Failed to initialize IL2CPP telemetry: {error}"));
        return InitResult::Error;
    }

    if let Err(error) = telemetry::start_sampler() {
        hachimi::log_error(error);
        return InitResult::Error;
    }

    if let Err(error) = icon::start() {
        hachimi::log_warn(format!(
            "Character icons are disabled; text RPC remains active: {error}"
        ));
    }

    if let Err(error) = start_rpc_worker() {
        hachimi::log_error(error);
        return InitResult::Error;
    }

    hachimi::log_info("Horse Girl RPC initialized with live WorkSingleModeCharaData telemetry");

    InitResult::Ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discord_state_format() {
        let stats = TrainingStats {
            speed: 425,
            stamina: 310,
            power: 285,
            guts: 190,
            wit: 350,
        };

        assert_eq!(
            stats.to_discord_state(),
            "SPD 425 · STA 310 · POW 285 · GUT 190 · WIT 350"
        );
    }

    #[test]
    fn test_discord_text_layout() {
        let telemetry = telemetry::TrainingTelemetry {
            character_name: "Special Week".to_owned(),
            scenario_name: "Brighter Together\nOur Grand Concert".to_owned(),
            stats: TrainingStats {
                speed: 425,
                stamina: 310,
                power: 285,
                guts: 190,
                wit: 350,
            },
            icon: None,
            energy: Some(65),
            max_energy: Some(100),
            mood: Some(4),
            shop_state: None,
        };

        assert_eq!(telemetry.discord_activity_name(), "Training Special Week");
        assert_eq!(
            telemetry.discord_details(),
            "Brighter Together Our Grand Concert Scenario"
        );
        assert_eq!(
            telemetry.discord_state(),
            "SPD 425 · STA 310 · POW 285 · GUT 190 · WIT 350"
        );
    }

    #[test]
    fn test_large_text_shows_energy_and_mood() {
        let telemetry = telemetry::TrainingTelemetry {
            character_name: "Daitaku Helios".to_owned(),
            scenario_name: "Grand Live".to_owned(),
            stats: TrainingStats {
                speed: 425,
                stamina: 310,
                power: 285,
                guts: 190,
                wit: 350,
            },
            icon: None,
            energy: Some(65),
            max_energy: Some(100),
            mood: Some(4),
            shop_state: None,
        };
        assert_eq!(telemetry.discord_large_text(), "Energy 65/100 | Mood GOOD");
    }

    #[test]
    fn test_screen_details_map() {
        // Title is always Umamusume for non-career screens; details names the
        // screen. Unknown ViewIds stay unavailable.
        assert_eq!(telemetry::screen_details(100), Some("In Home Screen"));
        assert_eq!(telemetry::screen_details(1400), Some("Enhancing..."));
        assert_eq!(telemetry::screen_details(300), Some("Scouting..."));
        assert_eq!(telemetry::screen_details(3000), Some("Reading Story"));
        assert_eq!(telemetry::screen_details(8100), Some("In Events Screen"));
        assert_eq!(telemetry::screen_details(9999), None);
    }

    #[test]
    fn test_large_text_falls_back_to_character_name() {
        let telemetry = telemetry::TrainingTelemetry {
            character_name: "Special Week".to_owned(),
            scenario_name: "Grand Live".to_owned(),
            stats: TrainingStats {
                speed: 425,
                stamina: 310,
                power: 285,
                guts: 190,
                wit: 350,
            },
            icon: None,
            energy: None,
            max_energy: None,
            mood: None,
            shop_state: None,
        };
        assert_eq!(telemetry.discord_large_text(), "Special Week");
    }

    #[test]
    fn test_shop_state_replaces_stats() {
        let telemetry = telemetry::TrainingTelemetry {
            character_name: "Special Week".to_owned(),
            scenario_name: "Make a New Track".to_owned(),
            stats: TrainingStats {
                speed: 425,
                stamina: 310,
                power: 285,
                guts: 190,
                wit: 350,
            },
            icon: None,
            energy: Some(65),
            max_energy: Some(100),
            mood: Some(3),
            shop_state: Some("Buying Items"),
        };

        // Shop screens keep the character activity but swap the stats line.
        assert_eq!(telemetry.discord_activity_name(), "Training Special Week");
        assert_eq!(telemetry.discord_details(), "Make a New Track Scenario");
        assert_eq!(telemetry.discord_state(), "Buying Items");
        assert_eq!(telemetry.discord_large_text(), "Energy 65/100 | Mood NORMAL");
    }

    #[test]
    fn test_rpc_library_serializes_target_layout() {
        let payload = serde_json::to_value(
            activity::Activity::new()
                .name("Training Daitaku Helios")
                .activity_type(activity::ActivityType::Playing)
                .details("Grand Live Scenario")
                .state("SPD 425 · STA 310 · POW 285 · GUT 190 · WIT 350")
                .assets(
                    activity::Assets::new()
                        .large_image(
                            "https://umamusu.wiki/w/images/d/d5/Game_Playable_Icon_106501.png",
                        )
                        .large_text("Daitaku Helios"),
                ),
        )
        .expect("activity should serialize");

        assert_eq!(payload["name"], "Training Daitaku Helios");
        assert_eq!(payload["type"], 0);
        assert_eq!(payload["details"], "Grand Live Scenario");
        assert_eq!(
            payload["state"],
            "SPD 425 · STA 310 · POW 285 · GUT 190 · WIT 350"
        );
        assert_eq!(
            payload["assets"]["large_image"],
            "https://umamusu.wiki/w/images/d/d5/Game_Playable_Icon_106501.png"
        );
        assert_eq!(payload["assets"]["large_text"], "Daitaku Helios");
    }
}
