use crate::TrainingStats;
use crate::hachimi::{self, HachimiApi};
use std::ffi::{CStr, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const MAX_DISPLAY_STRING_UTF16_LENGTH: i32 = 512;

// SceneDefine.SceneId.SingleMode — the scene a career run lives in.
const SINGLE_MODE_SCENE_ID: i32 = 8;

const READ_STATUS_UNKNOWN: u8 = 0;
const READ_STATUS_UNAVAILABLE: u8 = 1;
const READ_STATUS_AVAILABLE: u8 = 2;
const READ_STATUS_ERROR: u8 = 3;

static ICON_TELEMETRY_ERROR_REPORTED: AtomicBool = AtomicBool::new(false);

type Il2CppRuntimeInvokeFn = unsafe extern "C" fn(
    method: *const c_void,
    object: *mut c_void,
    params: *mut *mut c_void,
    exception: *mut *mut c_void,
) -> *mut c_void;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct CharacterIconIdentity {
    pub character_id: i32,
    pub current_card_id: i32,
    pub selected_card_id: i32,
    pub base_card_id: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrainingTelemetry {
    pub character_name: String,
    pub scenario_name: String,
    pub stats: TrainingStats,
    pub icon: Option<CharacterIconIdentity>,
    // Career energy (WorkSingleModeCharaData HP). Max can exceed 100 via support
    // cards, career events, and scenario items, so it is never assumed to be 100.
    pub energy: Option<u32>,
    pub max_energy: Option<u32>,
    // Motivation, 1..=5. Mapped to AWFUL/BAD/NORMAL/GOOD/GREAT for display.
    pub mood: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GameSnapshot {
    Training(TrainingTelemetry),
    // Non-career screen. `title` is always "Umamusume"; `details` names the
    // screen ("In Home Screen", "Scouting...", "Reading Story", ...). The state
    // field carries finer location (Home tab, scout type, story chapter, event
    // name) when the game exposes it; otherwise it is empty.
    MainMenu {
        title: &'static str,
        details: &'static str,
        state: Option<String>,
    },
    Unavailable,
}

impl TrainingTelemetry {
    pub fn discord_activity_name(&self) -> String {
        format!("Training {}", self.character_name)
    }

    // Large-image hover text. Replaces the character name with the Energy/Mood
    // status line; falls back to the name when neither reading is available.
    pub fn discord_large_text(&self) -> String {
        let mut status = String::new();
        if let (Some(energy), Some(max_energy)) = (self.energy, self.max_energy) {
            status.push_str(&format!("Energy {energy}/{max_energy}"));
        }
        if let Some(mood) = self.mood.and_then(mood_name) {
            if !status.is_empty() {
                status.push_str(" | ");
            }
            status.push_str("Mood ");
            status.push_str(mood);
        }
        if status.is_empty() {
            self.character_name.clone()
        } else {
            status
        }
    }

    pub fn discord_details(&self) -> String {
        let scenario_name = self
            .scenario_name
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if scenario_name.to_ascii_lowercase().ends_with("scenario") {
            scenario_name
        } else {
            format!("{scenario_name} Scenario")
        }
    }
}

struct TelemetryRuntime {
    runtime_invoke: Il2CppRuntimeInvokeFn,
    work_data_manager_class: usize,
    master_data_manager_class: usize,
    get_single_mode: usize,
    get_is_exist_playing_data: usize,
    get_scenario_id: usize,
    get_character: usize,
    get_chara_id: usize,
    get_speed: usize,
    get_stamina: usize,
    get_power: usize,
    get_guts: usize,
    get_wiz: usize,
    get_chara_name: usize,
    get_hp: usize,
    get_max_hp: usize,
    get_motivation: usize,
    get_master_single_mode_scenario: usize,
    get_scenario: usize,
    get_scenario_name: usize,
    screen: Option<ScreenRuntime>,
    icon: Option<IconTelemetryRuntime>,
    screen_content: Option<ScreenContentRuntime>,
}

// Resolves SceneManager lazily at init but tolerates absence, so a game update
// that renames the scene API only drops menu detection rather than killing
// training telemetry.
struct ScreenRuntime {
    scene_manager_class: usize,
    get_current_scene_id: usize,
    get_current_view_id: usize,
}

struct IconTelemetryRuntime {
    get_card_id: usize,
    get_race_dress_id: usize,
    get_card_id_from_dress_id: usize,
    master_data_manager_class: usize,
    get_master_card_data: usize,
    get_base_card: usize,
    card_data_id_field: usize,
}

// Optional screen-content lookups (story chapter, event name). Everything here
// soft-fails: if any class/field/method is missing or the field APIs are absent,
// these features drop out without aborting plugin init.
struct ScreenContentRuntime {
    // Story event name.
    work_data_manager_class: usize,
    master_data_manager_class: usize,
    get_story_event_data: usize,
    get_story_event_id: usize,
    get_master_story_event_data: usize,
    get_story_event: usize,
    get_story_event_short_title: usize,
    // Main-story chapter/episode (field reads; requires optional field APIs).
    temp_data_class: usize,
    episode_data_field: usize,
    is_in_main_story_field: usize,
    episode_main_story_field: usize,
    main_story_part_id_field: usize,
    main_story_episode_index_field: usize,
}

impl TelemetryRuntime {
    fn resolve(api: &HachimiApi) -> Result<Self, String> {
        let runtime_invoke_pointer =
            unsafe { (api.il2cpp_resolve_symbol)(c"il2cpp_runtime_invoke".as_ptr()) };
        if runtime_invoke_pointer.is_null() {
            return Err("IL2CPP export is unavailable: il2cpp_runtime_invoke".to_owned());
        }

        let image = unsafe { (api.il2cpp_get_assembly_image)(c"umamusume.dll".as_ptr()) };
        if image.is_null() {
            return Err("IL2CPP assembly is unavailable: umamusume.dll".to_owned());
        }

        let work_data_manager_class = require_class(api, image, c"Gallop", c"WorkDataManager")?;
        let master_data_manager_class = require_class(api, image, c"Gallop", c"MasterDataManager")?;
        let work_single_mode_data_class =
            require_class(api, image, c"Gallop", c"WorkSingleModeData")?;
        let work_single_mode_chara_data_class =
            require_class(api, image, c"Gallop", c"WorkSingleModeCharaData")?;
        let master_data_util_class = require_class(api, image, c"Gallop", c"MasterDataUtil")?;
        let master_single_mode_scenario_class =
            require_class(api, image, c"Gallop", c"MasterSingleModeScenario")?;
        let single_mode_scenario_class = require_nested_class(
            api,
            master_single_mode_scenario_class,
            c"SingleModeScenario",
            "Gallop.MasterSingleModeScenario.SingleModeScenario",
        )?;
        let icon = match IconTelemetryRuntime::resolve(
            api,
            image,
            work_single_mode_chara_data_class,
            master_data_manager_class,
        ) {
            Ok(runtime) => Some(runtime),
            Err(error) => {
                hachimi::log_warn(format!(
                    "Character icon telemetry is unavailable; text RPC remains active: {error}"
                ));
                None
            }
        };
        let screen = match ScreenRuntime::resolve(api, image) {
            Ok(runtime) => Some(runtime),
            Err(error) => {
                hachimi::log_warn(format!(
                    "Main menu detection is unavailable; training RPC remains active: {error}"
                ));
                None
            }
        };

        Ok(Self {
            // Hachimi resolves IL2CPP exports from the loaded GameAssembly module.
            runtime_invoke: unsafe {
                std::mem::transmute::<*mut c_void, Il2CppRuntimeInvokeFn>(runtime_invoke_pointer)
            },
            work_data_manager_class,
            master_data_manager_class,
            get_single_mode: require_method(
                api,
                work_data_manager_class,
                c"get_SingleMode",
                0,
                "Gallop.WorkDataManager.get_SingleMode",
            )?,
            get_is_exist_playing_data: require_method(
                api,
                work_single_mode_data_class,
                c"get_IsExistPlayingData",
                0,
                "Gallop.WorkSingleModeData.get_IsExistPlayingData",
            )?,
            get_scenario_id: require_method(
                api,
                work_single_mode_data_class,
                c"GetScenarioId",
                0,
                "Gallop.WorkSingleModeData.GetScenarioId",
            )?,
            get_character: require_method(
                api,
                work_single_mode_data_class,
                c"get_Character",
                0,
                "Gallop.WorkSingleModeData.get_Character",
            )?,
            get_chara_id: require_method(
                api,
                work_single_mode_chara_data_class,
                c"get_CharaId",
                0,
                "Gallop.WorkSingleModeCharaData.get_CharaId",
            )?,
            get_speed: require_method(
                api,
                work_single_mode_chara_data_class,
                c"get_Speed",
                0,
                "Gallop.WorkSingleModeCharaData.get_Speed",
            )?,
            get_stamina: require_method(
                api,
                work_single_mode_chara_data_class,
                c"get_Stamina",
                0,
                "Gallop.WorkSingleModeCharaData.get_Stamina",
            )?,
            get_power: require_method(
                api,
                work_single_mode_chara_data_class,
                c"get_Power",
                0,
                "Gallop.WorkSingleModeCharaData.get_Power",
            )?,
            get_guts: require_method(
                api,
                work_single_mode_chara_data_class,
                c"get_Guts",
                0,
                "Gallop.WorkSingleModeCharaData.get_Guts",
            )?,
            get_wiz: require_method(
                api,
                work_single_mode_chara_data_class,
                c"get_Wiz",
                0,
                "Gallop.WorkSingleModeCharaData.get_Wiz",
            )?,
            get_chara_name: require_method(
                api,
                master_data_util_class,
                c"GetCharaNameByCharaId",
                1,
                "Gallop.MasterDataUtil.GetCharaNameByCharaId",
            )?,
            get_hp: require_method(
                api,
                work_single_mode_chara_data_class,
                c"get_Hp",
                0,
                "Gallop.WorkSingleModeCharaData.get_Hp",
            )?,
            get_max_hp: require_method(
                api,
                work_single_mode_chara_data_class,
                c"get_MaxHp",
                0,
                "Gallop.WorkSingleModeCharaData.get_MaxHp",
            )?,
            get_motivation: require_method(
                api,
                work_single_mode_chara_data_class,
                c"get_Motivation",
                0,
                "Gallop.WorkSingleModeCharaData.get_Motivation",
            )?,
            get_master_single_mode_scenario: require_method(
                api,
                master_data_manager_class,
                c"get_masterSingleModeScenario",
                0,
                "Gallop.MasterDataManager.get_masterSingleModeScenario",
            )?,
            get_scenario: require_method(
                api,
                master_single_mode_scenario_class,
                c"Get",
                1,
                "Gallop.MasterSingleModeScenario.Get",
            )?,
            get_scenario_name: require_method(
                api,
                single_mode_scenario_class,
                c"GetName",
                1,
                "Gallop.MasterSingleModeScenario.SingleModeScenario.GetName",
            )?,
            icon,
            screen,
            // Story chapter and event name are optional: resolving them must
            // never abort plugin init.
            screen_content: ScreenContentRuntime::resolve(
                api,
                image,
                work_data_manager_class,
                master_data_manager_class,
            ),
        })
    }

    fn read(&self, api: &HachimiApi) -> Result<GameSnapshot, String> {
        // Read scene/view first. IsExistPlayingData is a saved flag that stays
        // true after leaving a career, so screen state decides whether training
        // stats are shown at all.
        let (scene_id, view_id) = match self.screen.as_ref() {
            Some(screen) => (
                screen.current_scene_id(self, api),
                screen.current_view_id(self, api),
            ),
            None => (None, None),
        };

        // Career runs live in the SingleMode scene, gameplay view range only.
        // ViewId 1000 is the pre-run setup wizard (route/card/succession select),
        // which is planning, not an active run.
        let in_career = scene_id == Some(SINGLE_MODE_SCENE_ID)
            && matches!(view_id, Some(id) if (1100..=1400).contains(&id));

        if !in_career {
            return Ok(describe_screen(view_id, api));
        }

        let work_data_manager = unsafe {
            (api.il2cpp_get_singleton_like_instance)(self.work_data_manager_class as *mut c_void)
        };
        if work_data_manager.is_null() {
            return Ok(GameSnapshot::Unavailable);
        }

        let single_mode = self.invoke_object(
            self.get_single_mode,
            work_data_manager,
            &mut [],
            "WorkDataManager.get_SingleMode",
        )?;
        if single_mode.is_null() {
            return Ok(GameSnapshot::Unavailable);
        }

        if !self.invoke_bool(
            api,
            self.get_is_exist_playing_data,
            single_mode,
            "WorkSingleModeData.get_IsExistPlayingData",
        )? {
            return Ok(GameSnapshot::Unavailable);
        }

        let scenario_id = self.invoke_i32(
            api,
            self.get_scenario_id,
            single_mode,
            "WorkSingleModeData.GetScenarioId",
        )?;
        if scenario_id <= 0 {
            return Ok(GameSnapshot::Unavailable);
        }
        let scenario_name = self.invoke_scenario_name(api, scenario_id)?;
        if scenario_name.trim().is_empty() {
            return Ok(GameSnapshot::Unavailable);
        }

        let character = self.invoke_object(
            self.get_character,
            single_mode,
            &mut [],
            "WorkSingleModeData.get_Character",
        )?;
        if character.is_null() {
            return Ok(GameSnapshot::Unavailable);
        }

        let chara_id = self.invoke_i32(
            api,
            self.get_chara_id,
            character,
            "WorkSingleModeCharaData.get_CharaId",
        )?;
        if chara_id <= 0 {
            return Ok(GameSnapshot::Unavailable);
        }

        let character_name = self.invoke_character_name(api, chara_id)?;
        if character_name.trim().is_empty() {
            return Ok(GameSnapshot::Unavailable);
        }

        let mood = self
            .invoke_stat(
                api,
                self.get_motivation,
                character,
                "WorkSingleModeCharaData.get_Motivation",
            )
            .ok()
            .filter(|motivation| (1..=5).contains(motivation));

        let energy = self.invoke_stat(
            api,
            self.get_hp,
            character,
            "WorkSingleModeCharaData.get_Hp",
        );
        let max_energy = self.invoke_stat(
            api,
            self.get_max_hp,
            character,
            "WorkSingleModeCharaData.get_MaxHp",
        );
        // A missing or invalid reading is not fatal; the Energy display is simply
        // omitted rather than dropping the whole Training activity.
        let (energy, max_energy) = match (energy, max_energy) {
            (Ok(energy), Ok(max_energy)) if energy > 0 && max_energy > 0 => {
                (Some(energy), Some(max_energy))
            }
            _ => (None, None),
        };

        let icon = self.icon.as_ref().and_then(|runtime| {
            match runtime.read(self, api, character, chara_id) {
                Ok(identity) => {
                    ICON_TELEMETRY_ERROR_REPORTED.store(false, Ordering::Release);
                    Some(identity)
                }
                Err(error) => {
                    if !ICON_TELEMETRY_ERROR_REPORTED.swap(true, Ordering::AcqRel) {
                        hachimi::log_warn(format!(
                            "Character icon detection failed; text RPC remains active: {error}"
                        ));
                    }
                    None
                }
            }
        });

        let stats = TrainingStats {
            speed: self.invoke_stat(
                api,
                self.get_speed,
                character,
                "WorkSingleModeCharaData.get_Speed",
            )?,
            stamina: self.invoke_stat(
                api,
                self.get_stamina,
                character,
                "WorkSingleModeCharaData.get_Stamina",
            )?,
            power: self.invoke_stat(
                api,
                self.get_power,
                character,
                "WorkSingleModeCharaData.get_Power",
            )?,
            guts: self.invoke_stat(
                api,
                self.get_guts,
                character,
                "WorkSingleModeCharaData.get_Guts",
            )?,
            // The game names its intelligence stat "Wiz"; Discord displays it as WIT.
            wit: self.invoke_stat(
                api,
                self.get_wiz,
                character,
                "WorkSingleModeCharaData.get_Wiz",
            )?,
        };

        Ok(GameSnapshot::Training(TrainingTelemetry {
            character_name: character_name.trim().to_owned(),
            scenario_name: scenario_name.trim().to_owned(),
            stats,
            icon,
            energy,
            max_energy,
            mood,
        }))
    }

    fn invoke(
        &self,
        method: usize,
        object: *mut c_void,
        params: &mut [*mut c_void],
        operation: &'static str,
    ) -> Result<*mut c_void, String> {
        let mut exception = ptr::null_mut();
        let params_pointer = if params.is_empty() {
            ptr::null_mut()
        } else {
            params.as_mut_ptr()
        };

        let result = unsafe {
            (self.runtime_invoke)(
                method as *const c_void,
                object,
                params_pointer,
                &mut exception,
            )
        };

        if exception.is_null() {
            Ok(result)
        } else {
            Err(format!("managed exception in {operation}"))
        }
    }

    fn invoke_object(
        &self,
        method: usize,
        object: *mut c_void,
        params: &mut [*mut c_void],
        operation: &'static str,
    ) -> Result<*mut c_void, String> {
        self.invoke(method, object, params, operation)
    }

    fn invoke_i32(
        &self,
        api: &HachimiApi,
        method: usize,
        object: *mut c_void,
        operation: &'static str,
    ) -> Result<i32, String> {
        self.invoke_i32_with_params(api, method, object, &mut [], operation)
    }

    fn invoke_i32_with_params(
        &self,
        api: &HachimiApi,
        method: usize,
        object: *mut c_void,
        params: &mut [*mut c_void],
        operation: &'static str,
    ) -> Result<i32, String> {
        let boxed = self.invoke(method, object, params, operation)?;
        if boxed.is_null() {
            return Err(format!("{operation} returned no value"));
        }

        let value = unsafe { (api.il2cpp_unbox)(boxed) };
        if value.is_null() {
            return Err(format!("{operation} returned an invalid boxed value"));
        }

        Ok(unsafe { ptr::read_unaligned(value.cast::<i32>()) })
    }

    fn invoke_bool(
        &self,
        api: &HachimiApi,
        method: usize,
        object: *mut c_void,
        operation: &'static str,
    ) -> Result<bool, String> {
        let boxed = self.invoke(method, object, &mut [], operation)?;
        if boxed.is_null() {
            return Err(format!("{operation} returned no value"));
        }

        let value = unsafe { (api.il2cpp_unbox)(boxed) };
        if value.is_null() {
            return Err(format!("{operation} returned an invalid boxed value"));
        }

        Ok(unsafe { ptr::read_unaligned(value.cast::<u8>()) } != 0)
    }

    fn invoke_stat(
        &self,
        api: &HachimiApi,
        method: usize,
        character: *mut c_void,
        operation: &'static str,
    ) -> Result<u32, String> {
        let value = self.invoke_i32(api, method, character, operation)?;
        u32::try_from(value).map_err(|_| format!("{operation} returned a negative value"))
    }

    fn invoke_character_name(&self, api: &HachimiApi, chara_id: i32) -> Result<String, String> {
        let mut chara_id_argument = chara_id;
        let mut params = [(&mut chara_id_argument as *mut i32).cast::<c_void>()];
        let string = self.invoke(
            self.get_chara_name,
            ptr::null_mut(),
            &mut params,
            "MasterDataUtil.GetCharaNameByCharaId",
        )?;
        if string.is_null() {
            return Err(format!(
                "MasterDataUtil.GetCharaNameByCharaId returned null for character {chara_id}"
            ));
        }

        il2cpp_string_to_rust(api, string, "character name")
    }

    fn invoke_scenario_name(&self, api: &HachimiApi, scenario_id: i32) -> Result<String, String> {
        let master_data_manager = unsafe {
            (api.il2cpp_get_singleton_like_instance)(self.master_data_manager_class as *mut c_void)
        };
        if master_data_manager.is_null() {
            return Err("MasterDataManager singleton is unavailable".to_owned());
        }

        let master_scenarios = self.invoke_object(
            self.get_master_single_mode_scenario,
            master_data_manager,
            &mut [],
            "MasterDataManager.get_masterSingleModeScenario",
        )?;
        if master_scenarios.is_null() {
            return Err("masterSingleModeScenario is unavailable".to_owned());
        }

        let mut scenario_id_argument = scenario_id;
        let mut scenario_params = [(&mut scenario_id_argument as *mut i32).cast::<c_void>()];
        let scenario = self.invoke_object(
            self.get_scenario,
            master_scenarios,
            &mut scenario_params,
            "MasterSingleModeScenario.Get",
        )?;
        if scenario.is_null() {
            return Err(format!(
                "MasterSingleModeScenario.Get returned null for scenario {scenario_id}"
            ));
        }

        // GetName(true) deletes line breaks without inserting a space. Keep the original
        // whitespace here; discord_details() safely collapses it to single spaces.
        let mut remove_new_line = 0_u8;
        let mut name_params = [(&mut remove_new_line as *mut u8).cast::<c_void>()];
        let string = self.invoke(
            self.get_scenario_name,
            scenario,
            &mut name_params,
            "MasterSingleModeScenario.SingleModeScenario.GetName",
        )?;
        if string.is_null() {
            return Err(format!(
                "SingleModeScenario.GetName returned null for scenario {scenario_id}"
            ));
        }

        il2cpp_string_to_rust(api, string, "scenario name")
    }
}

impl IconTelemetryRuntime {
    fn resolve(
        api: &HachimiApi,
        image: *const c_void,
        work_single_mode_chara_data_class: usize,
        master_data_manager_class: usize,
    ) -> Result<Self, String> {
        let gallop_util_class = require_class(api, image, c"Gallop", c"GallopUtil")?;
        let master_card_data_class = require_class(api, image, c"Gallop", c"MasterCardData")?;
        let card_data_class = require_nested_class(
            api,
            master_card_data_class,
            c"CardData",
            "Gallop.MasterCardData.CardData",
        )?;

        Ok(Self {
            get_card_id: require_method(
                api,
                work_single_mode_chara_data_class,
                c"get_CardId",
                0,
                "Gallop.WorkSingleModeCharaData.get_CardId",
            )?,
            get_race_dress_id: require_method(
                api,
                work_single_mode_chara_data_class,
                c"GetRaceDressId",
                1,
                "Gallop.WorkSingleModeCharaData.GetRaceDressId",
            )?,
            get_card_id_from_dress_id: require_method(
                api,
                gallop_util_class,
                c"GetCardIdFromDressId",
                2,
                "Gallop.GallopUtil.GetCardIdFromDressId",
            )?,
            master_data_manager_class,
            get_master_card_data: require_method(
                api,
                master_data_manager_class,
                c"get_masterCardData",
                0,
                "Gallop.MasterDataManager.get_masterCardData",
            )?,
            get_base_card: require_method(
                api,
                master_card_data_class,
                c"GetCommonRareCardDataByCharaId",
                1,
                "Gallop.MasterCardData.GetCommonRareCardDataByCharaId",
            )?,
            card_data_id_field: require_field(
                api,
                card_data_class,
                c"Id",
                "Gallop.MasterCardData.CardData.Id",
            )?,
        })
    }

    fn read(
        &self,
        telemetry: &TelemetryRuntime,
        api: &HachimiApi,
        character: *mut c_void,
        character_id: i32,
    ) -> Result<CharacterIconIdentity, String> {
        let selected_card_id = telemetry.invoke_i32(
            api,
            self.get_card_id,
            character,
            "WorkSingleModeCharaData.get_CardId",
        )?;
        if selected_card_id <= 0 {
            return Err("active character has no valid card ID".to_owned());
        }

        let current_card_id = self
            .current_costume_card_id(telemetry, api, character, character_id)
            .filter(|card_id| *card_id > 0)
            .unwrap_or(selected_card_id);
        let base_card_id = self
            .base_card_id(telemetry, api, character_id)
            .filter(|card_id| *card_id > 0)
            .unwrap_or(selected_card_id);

        Ok(CharacterIconIdentity {
            character_id,
            current_card_id,
            selected_card_id,
            base_card_id,
        })
    }

    fn current_costume_card_id(
        &self,
        telemetry: &TelemetryRuntime,
        api: &HachimiApi,
        character: *mut c_void,
        character_id: i32,
    ) -> Option<i32> {
        let mut apply_dress_change = 1_u8;
        let mut dress_params = [(&mut apply_dress_change as *mut u8).cast::<c_void>()];
        let dress_id = telemetry
            .invoke_i32_with_params(
                api,
                self.get_race_dress_id,
                character,
                &mut dress_params,
                "WorkSingleModeCharaData.GetRaceDressId",
            )
            .ok()?;
        if dress_id <= 0 {
            return None;
        }

        let mut character_id_argument = character_id;
        let mut dress_id_argument = dress_id;
        let mut params = [
            (&mut character_id_argument as *mut i32).cast::<c_void>(),
            (&mut dress_id_argument as *mut i32).cast::<c_void>(),
        ];
        telemetry
            .invoke_i32_with_params(
                api,
                self.get_card_id_from_dress_id,
                ptr::null_mut(),
                &mut params,
                "GallopUtil.GetCardIdFromDressId",
            )
            .ok()
    }

    fn base_card_id(
        &self,
        telemetry: &TelemetryRuntime,
        api: &HachimiApi,
        character_id: i32,
    ) -> Option<i32> {
        let master_data_manager = unsafe {
            (api.il2cpp_get_singleton_like_instance)(self.master_data_manager_class as *mut c_void)
        };
        if master_data_manager.is_null() {
            return None;
        }

        let master_card_data = telemetry
            .invoke_object(
                self.get_master_card_data,
                master_data_manager,
                &mut [],
                "MasterDataManager.get_masterCardData",
            )
            .ok()?;
        if master_card_data.is_null() {
            return None;
        }

        let mut character_id_argument = character_id;
        let mut params = [(&mut character_id_argument as *mut i32).cast::<c_void>()];
        let card_data = telemetry
            .invoke_object(
                self.get_base_card,
                master_card_data,
                &mut params,
                "MasterCardData.GetCommonRareCardDataByCharaId",
            )
            .ok()?;
        if card_data.is_null() {
            return None;
        }

        let mut card_id = 0_i32;
        let get_field_value = api.il2cpp_get_field_value?;
        unsafe {
            get_field_value(
                card_data,
                self.card_data_id_field as *mut c_void,
                (&mut card_id as *mut i32).cast::<c_void>(),
            );
        }
        Some(card_id)
    }
}

impl ScreenRuntime {
    fn resolve(api: &HachimiApi, image: *const c_void) -> Result<Self, String> {
        // SceneManager lives in the Gallop namespace.
        let scene_manager_class = require_class(api, image, c"Gallop", c"SceneManager")?;
        let get_current_view_id = require_method(
            api,
            scene_manager_class,
            c"GetCurrentViewId",
            0,
            "SceneManager.GetCurrentViewId",
        )?;
        let get_current_scene_id = require_method(
            api,
            scene_manager_class,
            c"GetCurrentSceneId",
            0,
            "SceneManager.GetCurrentSceneId",
        )?;

        Ok(Self {
            scene_manager_class,
            get_current_scene_id,
            get_current_view_id,
        })
    }

    // None = not a recognized screen. Some(view_id) = recognized, caller decides
    // how to describe it.
    fn current_view_id(&self, telemetry: &TelemetryRuntime, api: &HachimiApi) -> Option<i32> {
        let scene_manager = unsafe {
            (api.il2cpp_get_singleton_like_instance)(self.scene_manager_class as *mut c_void)
        };
        if scene_manager.is_null() {
            return None;
        }

        telemetry
            .invoke_i32(
                api,
                self.get_current_view_id,
                scene_manager,
                "SceneManager.GetCurrentViewId",
            )
            .ok()
    }

    fn current_scene_id(&self, telemetry: &TelemetryRuntime, api: &HachimiApi) -> Option<i32> {
        let scene_manager = unsafe {
            (api.il2cpp_get_singleton_like_instance)(self.scene_manager_class as *mut c_void)
        };
        if scene_manager.is_null() {
            return None;
        }

        telemetry
            .invoke_i32(
                api,
                self.get_current_scene_id,
                scene_manager,
                "SceneManager.GetCurrentSceneId",
            )
            .ok()
    }
}

// Motivation is 1..=5; anything else is treated as missing.
fn mood_name(motivation: u32) -> Option<&'static str> {
    let name = match motivation {
        1 => "AWFUL",
        2 => "BAD",
        3 => "NORMAL",
        4 => "GOOD",
        5 => "GREAT",
        _ => return None,
    };
    Some(name)
}

// Maps a ViewId to a Discord-facing screen description. Anything not listed is
// treated as unavailable so detail pages and popups never leak. Title is always
// "Umamusume"; the details string names the screen.
pub(crate) fn screen_details(view_id: i32) -> Option<&'static str> {
    let details = match view_id {
        100 | 101 => "In Home Screen",
        1400 => "Enhancing...",
        3000 => "Reading Story",
        400 | 401 => "In Race Screen",
        300 => "Scouting...",
        200 => "Watching Live",
        5710 => "In Live Theater",
        5800 | 5810 => "In Circle",
        4000..=4090 => "In Team Stadium",
        5600 | 5610 | 5650 => "In Daily Race",
        5620 | 5630 => "In Legend Race",
        5900..=5902 => "In Champions Meeting",
        5730 => "In Shop",
        5400 => "Viewing Missions",
        8100 | 8101 => "In Events Screen",
        8500 => "In Map Event",
        5510 | 5530 | 5540 => "Browsing Support Cards",
        5225 | 5226 => "In Character Gallery",
        6000 | 7010..=7013 => "Playing Crane Game",
        6100..=6109 => "In Room Match",
        6200..=6207 => "In Practice Race",
        6300..=6303 => "In Training Challenge",
        6450..=6456 => "In Jobs",
        8250..=8263 => "In Team Building",
        8270..=8295 => "In Heroes",
        8310 | 8311 => "In Collect Event",
        8320..=8322 => "In Collect Raid",
        8350..=8352 => "In Factor Research",
        8360 | 8361 => "In Ultimate Race",
        8400 | 8401 => "In Campaign",
        8150..=8160 => "In Challenge Match",
        _ => return None,
    };
    Some(details)
}

impl ScreenContentRuntime {
    // Returns None when any piece is missing so the caller keeps working without
    // story/event detail.
    fn resolve(
        api: &HachimiApi,
        image: *const c_void,
        work_data_manager_class: usize,
        master_data_manager_class: usize,
    ) -> Option<Self> {
        let work_story_event_data_class = require_class(api, image, c"Gallop", c"WorkStoryEventData").ok()?;
        let master_story_event_data_class =
            require_class(api, image, c"Gallop", c"MasterStoryEventData").ok()?;
        let master_story_event_row_class = require_nested_class(
            api,
            master_story_event_data_class,
            c"StoryEventData",
            "Gallop.MasterStoryEventData.StoryEventData",
        )
        .ok()?;
        let temp_data_class = require_class(api, image, c"Gallop", c"TempData").ok()?;
        let episode_temp_data_class = require_nested_class(
            api,
            temp_data_class,
            c"EpisodeTempData",
            "Gallop.TempData.EpisodeTempData",
        )
        .ok()?;
        let main_story_data_class = require_nested_class(
            api,
            require_class(api, image, c"", c"MasterMainStoryData").ok()?,
            c"MainStoryData",
            "MasterMainStoryData.MainStoryData",
        )
        .ok()?;

        // Field lookups use the optional field APIs; bail out if absent.
        api.il2cpp_get_field_from_name?;
        api.il2cpp_get_field_value?;

        Some(Self {
            work_data_manager_class,
            master_data_manager_class,
            get_story_event_data: require_method(
                api,
                work_data_manager_class,
                c"get_StoryEventData",
                0,
                "Gallop.WorkDataManager.get_StoryEventData",
            )
            .ok()?,
            get_story_event_id: require_method(
                api,
                work_story_event_data_class,
                c"get_StoryEventId",
                0,
                "Gallop.WorkStoryEventData.get_StoryEventId",
            )
            .ok()?,
            get_master_story_event_data: require_method(
                api,
                master_data_manager_class,
                c"get_masterStoryEventData",
                0,
                "Gallop.MasterDataManager.get_masterStoryEventData",
            )
            .ok()?,
            get_story_event: require_method(
                api,
                master_story_event_data_class,
                c"Get",
                1,
                "Gallop.MasterStoryEventData.Get",
            )
            .ok()?,
            get_story_event_short_title: require_method(
                api,
                master_story_event_row_class,
                c"get_ShortTitle",
                0,
                "Gallop.MasterStoryEventData.StoryEventData.get_ShortTitle",
            )
            .ok()?,
            temp_data_class,
            episode_data_field: require_field(
                api,
                temp_data_class,
                c"EpisodeData",
                "Gallop.TempData.EpisodeData",
            )
            .ok()?,
            is_in_main_story_field: require_field(
                api,
                temp_data_class,
                c"IsInMainStory",
                "Gallop.TempData.IsInMainStory",
            )
            .ok()?,
            episode_main_story_field: require_field(
                api,
                episode_temp_data_class,
                c"CurrentMainStoryData",
                "Gallop.TempData.EpisodeTempData.CurrentMainStoryData",
            )
            .ok()?,
            main_story_part_id_field: require_field(
                api,
                main_story_data_class,
                c"PartId",
                "MasterMainStoryData.MainStoryData.PartId",
            )
            .ok()?,
            main_story_episode_index_field: require_field(
                api,
                main_story_data_class,
                c"EpisodeIndex",
                "MasterMainStoryData.MainStoryData.EpisodeIndex",
            )
            .ok()?,
        })
    }
}

// Reads the currently-playing Main Story from TempData (a Singleton, so
// reachable): "Chapter {PartId} Episode {EpisodeIndex}". Character and extra
// stories need additional master-data chains and are not resolved here; they
// fall through to the underlying screen. Returns None when no main story is
// playing or any link in the chain is unavailable.
fn fetch_story_state(api: &HachimiApi) -> Option<String> {
    let runtime = RUNTIME.get()?;
    let content = runtime.screen_content.as_ref()?;
    let get_field_value = api.il2cpp_get_field_value?;

    let temp_data = unsafe {
        (api.il2cpp_get_singleton_like_instance)(content.temp_data_class as *mut c_void)
    };
    if temp_data.is_null() {
        return None;
    }

    let mut in_main_story = false;
    unsafe {
        get_field_value(
            temp_data,
            content.is_in_main_story_field as *mut c_void,
            (&mut in_main_story as *mut bool).cast::<c_void>(),
        );
    }
    if !in_main_story {
        return None;
    }

    let mut episode_data: *mut c_void = ptr::null_mut();
    unsafe {
        get_field_value(
            temp_data,
            content.episode_data_field as *mut c_void,
            (&mut episode_data as *mut *mut c_void).cast::<c_void>(),
        );
    }
    if episode_data.is_null() {
        return None;
    }

    let mut main_story: *mut c_void = ptr::null_mut();
    unsafe {
        get_field_value(
            episode_data,
            content.episode_main_story_field as *mut c_void,
            (&mut main_story as *mut *mut c_void).cast::<c_void>(),
        );
    }
    if main_story.is_null() {
        return None;
    }

    let mut part_id = 0_i32;
    let mut episode_index = 0_i32;
    unsafe {
        get_field_value(
            main_story,
            content.main_story_part_id_field as *mut c_void,
            (&mut part_id as *mut i32).cast::<c_void>(),
        );
        get_field_value(
            main_story,
            content.main_story_episode_index_field as *mut c_void,
            (&mut episode_index as *mut i32).cast::<c_void>(),
        );
    }

    if part_id > 0 && episode_index > 0 {
        Some(format!("Chapter {part_id} Episode {episode_index}"))
    } else {
        None
    }
}

fn describe_screen(view_id: Option<i32>, api: &HachimiApi) -> GameSnapshot {
    const TITLE: &str = "Umamusume";

    // An active story playback outranks the underlying view: the game keeps the
    // hub ViewId while the story overlay runs, so TempData is the source of truth.
    if let Some(story) = fetch_story_state(api) {
        return GameSnapshot::MainMenu {
            title: TITLE,
            details: "Reading Story",
            state: Some(story),
        };
    }

    let view_id = match view_id {
        Some(id) => id,
        None => return GameSnapshot::Unavailable,
    };

    let details = match screen_details(view_id) {
        Some(details) => details,
        None => return GameSnapshot::Unavailable,
    };

    // Finer location where the game exposes it. Home tabs and scout type are not
    // reachable from the resolved singletons, so only Events carries a state.
    let state = match view_id {
        8100 | 8101 => fetch_story_event_name(api),
        _ => None,
    };

    GameSnapshot::MainMenu {
        title: TITLE,
        details,
        state,
    }
}

// WorkDataManager.get_StoryEventData().get_StoryEventId() -> MasterDataManager
// .get_masterStoryEventData().Get(id).get_ShortTitle(). Returns None when no
// event is active or any link in the chain is unavailable.
fn fetch_story_event_name(api: &HachimiApi) -> Option<String> {
    const OP: &str = "StoryEvent name";
    let runtime = RUNTIME.get()?;
    let content = runtime.screen_content.as_ref()?;

    let work_data_manager = unsafe {
        (api.il2cpp_get_singleton_like_instance)(content.work_data_manager_class as *mut c_void)
    };
    if work_data_manager.is_null() {
        return None;
    }

    let story_event_data = runtime
        .invoke_object(
            content.get_story_event_data,
            work_data_manager,
            &mut [],
            "WorkDataManager.get_StoryEventData",
        )
        .ok()?;
    if story_event_data.is_null() {
        return None;
    }

    let story_event_id = runtime
        .invoke_i32(
            api,
            content.get_story_event_id,
            story_event_data,
            "WorkStoryEventData.get_StoryEventId",
        )
        .ok()?;
    if story_event_id <= 0 {
        return None;
    }

    let master_data_manager = unsafe {
        (api.il2cpp_get_singleton_like_instance)(content.master_data_manager_class as *mut c_void)
    };
    if master_data_manager.is_null() {
        return None;
    }

    let master_story_events = runtime
        .invoke_object(
            content.get_master_story_event_data,
            master_data_manager,
            &mut [],
            "MasterDataManager.get_masterStoryEventData",
        )
        .ok()?;
    if master_story_events.is_null() {
        return None;
    }

    let mut id_argument = story_event_id;
    let mut params = [(&mut id_argument as *mut i32).cast::<c_void>()];
    let story_event = runtime
        .invoke(
            content.get_story_event,
            master_story_events,
            &mut params,
            "MasterStoryEventData.Get",
        )
        .ok()?;
    if story_event.is_null() {
        return None;
    }

    let short_title = runtime
        .invoke_object(
            content.get_story_event_short_title,
            story_event,
            &mut [],
            "MasterStoryEventData.StoryEventData.get_ShortTitle",
        )
        .ok()?;
    if short_title.is_null() {
        return None;
    }

    let name = il2cpp_string_to_rust(api, short_title, OP).ok()?;
    if name.trim().is_empty() {
        None
    } else {
        Some(name)
    }
}

fn il2cpp_string_to_rust(
    api: &HachimiApi,
    string: *mut c_void,
    display_name: &'static str,
) -> Result<String, String> {
    let length = unsafe { (api.il2cpp_string_length)(string) };
    if !(0..=MAX_DISPLAY_STRING_UTF16_LENGTH).contains(&length) {
        return Err(format!("invalid {display_name} length: {length}"));
    }
    if length == 0 {
        return Ok(String::new());
    }

    let chars = unsafe { (api.il2cpp_string_chars)(string) };
    if chars.is_null() {
        return Err(format!("{display_name} UTF-16 data is null"));
    }

    let utf16 = unsafe { std::slice::from_raw_parts(chars, length as usize) };
    String::from_utf16(utf16)
        .map_err(|error| format!("{display_name} is not valid UTF-16: {error}"))
}

static RUNTIME: OnceLock<TelemetryRuntime> = OnceLock::new();
static LATEST: Mutex<GameSnapshot> = Mutex::new(GameSnapshot::Unavailable);
static SAMPLE_PENDING: AtomicBool = AtomicBool::new(false);
static READ_STATUS: AtomicU8 = AtomicU8::new(READ_STATUS_UNKNOWN);

pub fn initialize(api: &HachimiApi) -> Result<(), String> {
    let runtime = TelemetryRuntime::resolve(api)?;
    RUNTIME
        .set(runtime)
        .map_err(|_| "IL2CPP telemetry runtime is already initialized".to_owned())
}

pub fn start_sampler() -> Result<(), String> {
    let Some(api) = hachimi::api().copied() else {
        return Err("Hachimi API is not initialized".to_owned());
    };

    let main_thread = unsafe { (api.il2cpp_get_main_thread)() };
    if main_thread.is_null() {
        return Err("Hachimi could not locate Unity's main IL2CPP thread".to_owned());
    }

    thread::Builder::new()
        .name("HorseGirlTelemetry".into())
        .spawn(move || sampler_loop(api))
        .map(|_| ())
        .map_err(|error| format!("failed to start telemetry sampler: {error}"))
}

pub fn snapshot() -> GameSnapshot {
    match LATEST.lock() {
        Ok(latest) => latest.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

fn sampler_loop(api: HachimiApi) {
    loop {
        if SAMPLE_PENDING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let main_thread = unsafe { (api.il2cpp_get_main_thread)() };
            if main_thread.is_null() {
                report_read_error("Unity's main IL2CPP thread became unavailable".to_owned());
                SAMPLE_PENDING.store(false, Ordering::Release);
            } else {
                unsafe {
                    (api.il2cpp_schedule_on_thread)(main_thread, collect_on_main_thread);
                }
            }
        }

        thread::sleep(SAMPLE_INTERVAL);
    }
}

unsafe extern "C" fn collect_on_main_thread() {
    struct PendingSampleGuard;

    impl Drop for PendingSampleGuard {
        fn drop(&mut self) {
            SAMPLE_PENDING.store(false, Ordering::Release);
        }
    }

    let _pending_guard = PendingSampleGuard;
    let result = catch_unwind(AssertUnwindSafe(read_on_main_thread));

    match result {
        Ok(Ok(snapshot)) => store_snapshot(snapshot),
        Ok(Err(error)) => report_read_error(error),
        Err(_) => report_read_error("panic while reading IL2CPP telemetry".to_owned()),
    }
}

fn read_on_main_thread() -> Result<GameSnapshot, String> {
    let api = hachimi::api().ok_or_else(|| "Hachimi API is unavailable".to_owned())?;
    let runtime = RUNTIME
        .get()
        .ok_or_else(|| "IL2CPP telemetry runtime is unavailable".to_owned())?;
    runtime.read(api)
}

fn store_snapshot(snapshot: GameSnapshot) {
    let new_status = if matches!(snapshot, GameSnapshot::Training(_)) {
        READ_STATUS_AVAILABLE
    } else {
        READ_STATUS_UNAVAILABLE
    };
    let previous_status = READ_STATUS.swap(new_status, Ordering::AcqRel);

    match LATEST.lock() {
        Ok(mut latest) => *latest = snapshot.clone(),
        Err(poisoned) => *poisoned.into_inner() = snapshot.clone(),
    }

    if previous_status != new_status {
        if let GameSnapshot::Training(snapshot) = &snapshot {
            hachimi::log_info(format!(
                "Live training telemetry available for {}",
                snapshot.character_name
            ));
        } else if previous_status == READ_STATUS_AVAILABLE {
            hachimi::log_info("No active career; Discord training activity cleared");
        }
    }
}

fn report_read_error(error: String) {
    match LATEST.lock() {
        Ok(mut latest) => *latest = GameSnapshot::Unavailable,
        Err(poisoned) => *poisoned.into_inner() = GameSnapshot::Unavailable,
    }

    if READ_STATUS.swap(READ_STATUS_ERROR, Ordering::AcqRel) != READ_STATUS_ERROR {
        hachimi::log_warn(format!("Training telemetry unavailable: {error}"));
    }
}

fn require_class(
    api: &HachimiApi,
    image: *const c_void,
    namespace: &CStr,
    class_name: &CStr,
) -> Result<usize, String> {
    let class = unsafe { (api.il2cpp_get_class)(image, namespace.as_ptr(), class_name.as_ptr()) };
    if class.is_null() {
        Err(format!(
            "IL2CPP class is unavailable: {}.{}",
            namespace.to_string_lossy(),
            class_name.to_string_lossy()
        ))
    } else {
        Ok(class as usize)
    }
}

fn require_method(
    api: &HachimiApi,
    class: usize,
    method_name: &CStr,
    argument_count: i32,
    display_name: &'static str,
) -> Result<usize, String> {
    let method = unsafe {
        (api.il2cpp_get_method)(class as *mut c_void, method_name.as_ptr(), argument_count)
    };
    if method.is_null() {
        Err(format!("IL2CPP method is unavailable: {display_name}"))
    } else {
        Ok(method as usize)
    }
}

fn require_nested_class(
    api: &HachimiApi,
    parent_class: usize,
    class_name: &CStr,
    display_name: &'static str,
) -> Result<usize, String> {
    let class =
        unsafe { (api.il2cpp_find_nested_class)(parent_class as *mut c_void, class_name.as_ptr()) };
    if class.is_null() {
        Err(format!(
            "IL2CPP nested class is unavailable: {display_name}"
        ))
    } else {
        Ok(class as usize)
    }
}

fn require_field(
    api: &HachimiApi,
    class: usize,
    field_name: &CStr,
    display_name: &'static str,
) -> Result<usize, String> {
    let get_field = api
        .il2cpp_get_field_from_name
        .ok_or_else(|| "Hachimi field lookup API is unavailable".to_owned())?;
    let field = unsafe { get_field(class as *mut c_void, field_name.as_ptr()) };
    if field.is_null() {
        Err(format!("IL2CPP field is unavailable: {display_name}"))
    } else {
        Ok(field as usize)
    }
}
