use std::ffi::{CString, c_char, c_void};
use std::sync::OnceLock;

pub type HachimiGetApiFn = extern "C" fn(name: *const c_char) -> *mut c_void;

pub type LogFn = unsafe extern "C" fn(level: i32, target: *const c_char, message: *const c_char);
pub type Il2CppResolveSymbolFn = unsafe extern "C" fn(name: *const c_char) -> *mut c_void;
pub type Il2CppGetAssemblyImageFn =
    unsafe extern "C" fn(assembly_name: *const c_char) -> *const c_void;
pub type Il2CppGetClassFn = unsafe extern "C" fn(
    image: *const c_void,
    namespace: *const c_char,
    class_name: *const c_char,
) -> *mut c_void;
pub type Il2CppGetMethodFn =
    unsafe extern "C" fn(class: *mut c_void, name: *const c_char, args_count: i32) -> *const c_void;
pub type Il2CppFindNestedClassFn =
    unsafe extern "C" fn(class: *mut c_void, name: *const c_char) -> *mut c_void;
pub type Il2CppGetFieldFromNameFn =
    unsafe extern "C" fn(class: *mut c_void, name: *const c_char) -> *mut c_void;
pub type Il2CppGetFieldValueFn =
    unsafe extern "C" fn(object: *mut c_void, field: *mut c_void, out_value: *mut c_void);
pub type Il2CppUnboxFn = unsafe extern "C" fn(object: *mut c_void) -> *mut c_void;
pub type Il2CppGetSingletonLikeInstanceFn = unsafe extern "C" fn(class: *mut c_void) -> *mut c_void;
pub type Il2CppStringCharsFn = unsafe extern "C" fn(string: *mut c_void) -> *mut u16;
pub type Il2CppStringLengthFn = unsafe extern "C" fn(string: *mut c_void) -> i32;
pub type Il2CppGetMainThreadFn = unsafe extern "C" fn() -> *mut c_void;
pub type MainThreadCallback = unsafe extern "C" fn();
pub type Il2CppScheduleOnThreadFn =
    unsafe extern "C" fn(thread: *mut c_void, callback: MainThreadCallback);

#[derive(Clone, Copy)]
pub struct HachimiApi {
    pub log: LogFn,
    pub il2cpp_resolve_symbol: Il2CppResolveSymbolFn,
    pub il2cpp_get_assembly_image: Il2CppGetAssemblyImageFn,
    pub il2cpp_get_class: Il2CppGetClassFn,
    pub il2cpp_get_method: Il2CppGetMethodFn,
    pub il2cpp_find_nested_class: Il2CppFindNestedClassFn,
    pub il2cpp_get_field_from_name: Option<Il2CppGetFieldFromNameFn>,
    pub il2cpp_get_field_value: Option<Il2CppGetFieldValueFn>,
    pub il2cpp_unbox: Il2CppUnboxFn,
    pub il2cpp_get_singleton_like_instance: Il2CppGetSingletonLikeInstanceFn,
    pub il2cpp_string_chars: Il2CppStringCharsFn,
    pub il2cpp_string_length: Il2CppStringLengthFn,
    pub il2cpp_get_main_thread: Il2CppGetMainThreadFn,
    pub il2cpp_schedule_on_thread: Il2CppScheduleOnThreadFn,
}

impl HachimiApi {
    pub fn resolve(get_api: HachimiGetApiFn) -> Result<Self, String> {
        macro_rules! required_api {
            ($name:literal, $type:ty) => {{
                let name = CString::new($name).expect("static API name cannot contain NUL");
                let pointer = get_api(name.as_ptr());
                if pointer.is_null() {
                    return Err(format!("required Hachimi API is unavailable: {}", $name));
                }

                // Hachimi's v3 API explicitly returns function pointers as void pointers.
                unsafe { std::mem::transmute::<*mut c_void, $type>(pointer) }
            }};
        }
        macro_rules! optional_api {
            ($name:literal, $type:ty) => {{
                let name = CString::new($name).expect("static API name cannot contain NUL");
                let pointer = get_api(name.as_ptr());
                (!pointer.is_null())
                    .then(|| unsafe { std::mem::transmute::<*mut c_void, $type>(pointer) })
            }};
        }

        Ok(Self {
            log: required_api!("log", LogFn),
            il2cpp_resolve_symbol: required_api!("il2cpp_resolve_symbol", Il2CppResolveSymbolFn),
            il2cpp_get_assembly_image: required_api!(
                "il2cpp_get_assembly_image",
                Il2CppGetAssemblyImageFn
            ),
            il2cpp_get_class: required_api!("il2cpp_get_class", Il2CppGetClassFn),
            il2cpp_get_method: required_api!("il2cpp_get_method", Il2CppGetMethodFn),
            il2cpp_find_nested_class: required_api!(
                "il2cpp_find_nested_class",
                Il2CppFindNestedClassFn
            ),
            il2cpp_get_field_from_name: optional_api!(
                "il2cpp_get_field_from_name",
                Il2CppGetFieldFromNameFn
            ),
            il2cpp_get_field_value: optional_api!("il2cpp_get_field_value", Il2CppGetFieldValueFn),
            il2cpp_unbox: required_api!("il2cpp_unbox", Il2CppUnboxFn),
            il2cpp_get_singleton_like_instance: required_api!(
                "il2cpp_get_singleton_like_instance",
                Il2CppGetSingletonLikeInstanceFn
            ),
            il2cpp_string_chars: required_api!("il2cpp_string_chars", Il2CppStringCharsFn),
            il2cpp_string_length: required_api!("il2cpp_string_length", Il2CppStringLengthFn),
            il2cpp_get_main_thread: required_api!("il2cpp_get_main_thread", Il2CppGetMainThreadFn),
            il2cpp_schedule_on_thread: required_api!(
                "il2cpp_schedule_on_thread",
                Il2CppScheduleOnThreadFn
            ),
        })
    }
}

static API: OnceLock<HachimiApi> = OnceLock::new();

pub fn install(api: HachimiApi) -> Result<(), &'static str> {
    API.set(api).map_err(|_| "Hachimi API is already installed")
}

pub fn api() -> Option<&'static HachimiApi> {
    API.get()
}

pub fn log(level: i32, message: impl AsRef<str>) {
    let Some(api) = api() else {
        return;
    };

    let sanitized = message.as_ref().replace('\0', " ");
    let Ok(message) = CString::new(sanitized) else {
        return;
    };

    unsafe {
        (api.log)(level, c"HorseGirlRPC".as_ptr(), message.as_ptr());
    }
}

pub fn log_error(message: impl AsRef<str>) {
    log(1, message);
}

pub fn log_warn(message: impl AsRef<str>) {
    log(2, message);
}

pub fn log_info(message: impl AsRef<str>) {
    log(3, message);
}
