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
}

impl TrainingTelemetry {
    pub fn discord_activity_name(&self) -> String {
        format!("Training {}", self.character_name)
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
    get_master_single_mode_scenario: usize,
    get_scenario: usize,
    get_scenario_name: usize,
    icon: Option<IconTelemetryRuntime>,
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
        })
    }

    fn read(&self, api: &HachimiApi) -> Result<Option<TrainingTelemetry>, String> {
        let work_data_manager = unsafe {
            (api.il2cpp_get_singleton_like_instance)(self.work_data_manager_class as *mut c_void)
        };
        if work_data_manager.is_null() {
            return Ok(None);
        }

        let single_mode = self.invoke_object(
            self.get_single_mode,
            work_data_manager,
            &mut [],
            "WorkDataManager.get_SingleMode",
        )?;
        if single_mode.is_null() {
            return Ok(None);
        }

        if !self.invoke_bool(
            api,
            self.get_is_exist_playing_data,
            single_mode,
            "WorkSingleModeData.get_IsExistPlayingData",
        )? {
            return Ok(None);
        }

        let scenario_id = self.invoke_i32(
            api,
            self.get_scenario_id,
            single_mode,
            "WorkSingleModeData.GetScenarioId",
        )?;
        if scenario_id <= 0 {
            return Ok(None);
        }
        let scenario_name = self.invoke_scenario_name(api, scenario_id)?;
        if scenario_name.trim().is_empty() {
            return Ok(None);
        }

        let character = self.invoke_object(
            self.get_character,
            single_mode,
            &mut [],
            "WorkSingleModeData.get_Character",
        )?;
        if character.is_null() {
            return Ok(None);
        }

        let chara_id = self.invoke_i32(
            api,
            self.get_chara_id,
            character,
            "WorkSingleModeCharaData.get_CharaId",
        )?;
        if chara_id <= 0 {
            return Ok(None);
        }

        let character_name = self.invoke_character_name(api, chara_id)?;
        if character_name.trim().is_empty() {
            return Ok(None);
        }

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

        Ok(Some(TrainingTelemetry {
            character_name: character_name.trim().to_owned(),
            scenario_name: scenario_name.trim().to_owned(),
            stats,
            icon,
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
static LATEST: Mutex<Option<TrainingTelemetry>> = Mutex::new(None);
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

pub fn latest() -> Option<TrainingTelemetry> {
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

fn read_on_main_thread() -> Result<Option<TrainingTelemetry>, String> {
    let api = hachimi::api().ok_or_else(|| "Hachimi API is unavailable".to_owned())?;
    let runtime = RUNTIME
        .get()
        .ok_or_else(|| "IL2CPP telemetry runtime is unavailable".to_owned())?;
    runtime.read(api)
}

fn store_snapshot(snapshot: Option<TrainingTelemetry>) {
    let new_status = if snapshot.is_some() {
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
        if let Some(snapshot) = snapshot {
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
        Ok(mut latest) => *latest = None,
        Err(poisoned) => *poisoned.into_inner() = None,
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
