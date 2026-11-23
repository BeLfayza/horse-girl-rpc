use discord_rich_presence::{DiscordIpc, DiscordIpcClient, activity};
use serde::{Deserialize, Serialize};
use std::ffi::{CString, c_char, c_void};
use std::thread;
use std::time::Duration;

const DISCORD_APP_ID: &str = "1574266970763694212";

#[repr(i32)]
pub enum InitResult {
    Error = 0,
    Ok = 1,
}

type HachimiGetApiFn = extern "C" fn(name: *const c_char) -> *mut c_void;

type LogFn = unsafe extern "C" fn(level: i32, target: *const c_char, message: *const c_char);

#[derive(Debug, Clone, Serialize, Deserialize)]
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

fn start_rpc_worker() {
    let _ = thread::Builder::new()
        .name("HorseGirlRPC".into())
        .spawn(|| {
            loop {
                let mut client = DiscordIpcClient::new(DISCORD_APP_ID);

                if client.connect().is_ok() {
                    loop {
                        let stats = TrainingStats {
                            speed: 425,
                            stamina: 310,
                            power: 285,
                            guts: 190,
                            wit: 350,
                        };

                        let state = stats.to_discord_state();

                        let presence = activity::Activity::new()
                            .details("Training Special Week")
                            .state(&state);

                        if client.set_activity(presence).is_err() {
                            let _ = client.close();
                            break;
                        }

                        thread::sleep(Duration::from_secs(15));
                    }
                }

                thread::sleep(Duration::from_secs(5));
            }
        });
}

#[unsafe(no_mangle)]
pub extern "C" fn hachimi_init_v3(get_api: HachimiGetApiFn, version: i32) -> InitResult {
    if version < 3 {
        return InitResult::Error;
    }

    let api_name = CString::new("log").unwrap();
    let log_ptr = get_api(api_name.as_ptr());

    if log_ptr.is_null() {
        return InitResult::Error;
    }

    let log: LogFn = unsafe { std::mem::transmute(log_ptr) };

    let tag = CString::new("HorseGirlRPC").unwrap();
    let message = CString::new("Horse Girl RPC initialized successfully!").unwrap();

    unsafe {
        log(3, tag.as_ptr(), message.as_ptr());
    }

    start_rpc_worker();

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
}
