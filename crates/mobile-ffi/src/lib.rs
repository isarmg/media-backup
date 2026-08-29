#![allow(clippy::missing_safety_doc)]

use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::{CStr, CString},
    os::raw::c_char,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
};

use photo_backup_agent_core::{Agent, AgentConfig, EnqueueResource};
use serde_json::{json, Value};

static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);
static AGENTS: OnceLock<Mutex<HashMap<u64, Arc<Agent>>>> = OnceLock::new();

thread_local! {
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::new("").expect("empty CString"));
}

fn agents() -> &'static Mutex<HashMap<u64, Arc<Agent>>> {
    AGENTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn set_error(message: impl ToString) {
    let sanitized = message.to_string().replace('\0', " ");
    LAST_ERROR.with(|value| {
        *value.borrow_mut() = CString::new(sanitized).expect("sanitized CString");
    });
}

fn open_impl(database_path: &str, config_json: &str) -> Result<u64, String> {
    let config: AgentConfig =
        serde_json::from_str(config_json).map_err(|error| error.to_string())?;
    let agent = Agent::open(database_path, config).map_err(|error| error.to_string())?;
    let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
    agents()
        .lock()
        .map_err(|_| "agent registry mutex poisoned".to_owned())?
        .insert(handle, Arc::new(agent));
    Ok(handle)
}

fn with_agent<T>(
    handle: u64,
    operation: impl FnOnce(&Agent) -> Result<T, String>,
) -> Result<T, String> {
    let agent = agents()
        .lock()
        .map_err(|_| "agent registry mutex poisoned".to_owned())?
        .get(&handle)
        .cloned()
        .ok_or_else(|| "invalid agent handle".to_owned())?;
    operation(&agent)
}

fn envelope(result: Result<Value, String>) -> String {
    match result {
        Ok(value) => json!({"ok": true, "value": value}).to_string(),
        Err(error) => json!({"ok": false, "error": error}).to_string(),
    }
}

fn c_string(value: String) -> *mut c_char {
    CString::new(value.replace('\0', " "))
        .expect("sanitized CString")
        .into_raw()
}

unsafe fn read_c_string(value: *const c_char) -> Result<String, String> {
    if value.is_null() {
        return Err("null string pointer".to_owned());
    }
    CStr::from_ptr(value)
        .to_str()
        .map(str::to_owned)
        .map_err(|error| error.to_string())
}

#[no_mangle]
pub unsafe extern "C" fn pb_open(database_path: *const c_char, config_json: *const c_char) -> u64 {
    let result = read_c_string(database_path)
        .and_then(|path| read_c_string(config_json).and_then(|config| open_impl(&path, &config)));
    match result {
        Ok(handle) => handle,
        Err(error) => {
            set_error(error);
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn pb_close(handle: u64) {
    if let Ok(mut registry) = agents().lock() {
        registry.remove(&handle);
    }
}

#[no_mangle]
pub unsafe extern "C" fn pb_needs(
    handle: u64,
    source_asset_id: *const c_char,
    source_resource_id: *const c_char,
    modified_ms: i64,
) -> bool {
    let result = read_c_string(source_asset_id).and_then(|asset| {
        read_c_string(source_resource_id).and_then(|resource| {
            with_agent(handle, |agent| {
                agent
                    .needs_resource(&asset, &resource, modified_ms)
                    .map_err(|error| error.to_string())
            })
        })
    });
    match result {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            true
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn pb_enqueue(handle: u64, input_json: *const c_char) -> *mut c_char {
    let result = read_c_string(input_json).and_then(|input| {
        let parsed: EnqueueResource =
            serde_json::from_str(&input).map_err(|error| error.to_string())?;
        with_agent(handle, |agent| {
            agent
                .enqueue(parsed)
                .map(Value::String)
                .map_err(|error| error.to_string())
        })
    });
    c_string(envelope(result))
}

#[no_mangle]
pub unsafe extern "C" fn pb_next(handle: u64, staging_root: *const c_char) -> *mut c_char {
    let result = read_c_string(staging_root).and_then(|root| {
        with_agent(handle, |agent| {
            agent
                .next_prepared(root)
                .and_then(|value| serde_json::to_value(value).map_err(Into::into))
                .map_err(|error| error.to_string())
        })
    });
    c_string(envelope(result))
}

#[no_mangle]
pub unsafe extern "C" fn pb_mark_upload(
    handle: u64,
    job_id: *const c_char,
    upload_id: *const c_char,
) -> *mut c_char {
    let result = read_c_string(job_id).and_then(|job| {
        read_c_string(upload_id).and_then(|upload| {
            with_agent(handle, |agent| {
                agent
                    .mark_upload(&job, &upload)
                    .map(|_| Value::Null)
                    .map_err(|error| error.to_string())
            })
        })
    });
    c_string(envelope(result))
}

#[no_mangle]
pub unsafe extern "C" fn pb_mark_part(
    handle: u64,
    job_id: *const c_char,
    part_index: u32,
) -> *mut c_char {
    let result = read_c_string(job_id).and_then(|job| {
        with_agent(handle, |agent| {
            agent
                .mark_part_uploaded(&job, part_index)
                .map(|_| Value::Null)
                .map_err(|error| error.to_string())
        })
    });
    c_string(envelope(result))
}

#[no_mangle]
pub unsafe extern "C" fn pb_mark_complete(handle: u64, job_id: *const c_char) -> *mut c_char {
    let result = read_c_string(job_id).and_then(|job| {
        with_agent(handle, |agent| {
            agent
                .mark_complete(&job)
                .map(|_| Value::Null)
                .map_err(|error| error.to_string())
        })
    });
    c_string(envelope(result))
}

#[no_mangle]
pub unsafe extern "C" fn pb_mark_failed(
    handle: u64,
    job_id: *const c_char,
    error: *const c_char,
    retryable: bool,
) -> *mut c_char {
    let result = read_c_string(job_id).and_then(|job| {
        read_c_string(error).and_then(|message| {
            with_agent(handle, |agent| {
                agent
                    .mark_failed(&job, &message, retryable)
                    .map(|_| Value::Null)
                    .map_err(|error| error.to_string())
            })
        })
    });
    c_string(envelope(result))
}

#[no_mangle]
pub extern "C" fn pb_stats(handle: u64) -> *mut c_char {
    let result = with_agent(handle, |agent| {
        agent
            .stats()
            .and_then(|value| serde_json::to_value(value).map_err(Into::into))
            .map_err(|error| error.to_string())
    });
    c_string(envelope(result))
}

#[no_mangle]
pub extern "C" fn pb_last_error() -> *const c_char {
    LAST_ERROR.with(|value| value.borrow().as_ptr())
}

#[no_mangle]
pub unsafe extern "C" fn pb_string_free(value: *mut c_char) {
    if !value.is_null() {
        drop(CString::from_raw(value));
    }
}

#[cfg(target_os = "android")]
mod android {
    use super::*;
    use jni::{
        objects::{JClass, JString},
        sys::{jboolean, jint, jlong, jstring},
        JNIEnv,
    };

    fn java_string(env: &mut JNIEnv, value: JString) -> Result<String, String> {
        env.get_string(&value)
            .map(|value| value.into())
            .map_err(|error| error.to_string())
    }

    fn to_jstring(env: &mut JNIEnv, value: String) -> jstring {
        env.new_string(value)
            .map(|value| value.into_raw())
            .unwrap_or(std::ptr::null_mut())
    }

    #[no_mangle]
    pub extern "system" fn Java_com_example_photobackup_NativeBridge_nativeOpen(
        mut env: JNIEnv,
        _class: JClass,
        database_path: JString,
        config_json: JString,
    ) -> jlong {
        let result = java_string(&mut env, database_path).and_then(|path| {
            java_string(&mut env, config_json).and_then(|config| open_impl(&path, &config))
        });
        result.unwrap_or_else(|error| {
            set_error(error);
            0
        }) as jlong
    }

    #[no_mangle]
    pub extern "system" fn Java_com_example_photobackup_NativeBridge_nativeClose(
        _env: JNIEnv,
        _class: JClass,
        handle: jlong,
    ) {
        pb_close(handle as u64);
    }

    #[no_mangle]
    pub extern "system" fn Java_com_example_photobackup_NativeBridge_nativeNeeds(
        mut env: JNIEnv,
        _class: JClass,
        handle: jlong,
        asset: JString,
        resource: JString,
        modified_ms: jlong,
    ) -> jboolean {
        let result = java_string(&mut env, asset).and_then(|asset| {
            java_string(&mut env, resource).and_then(|resource| {
                with_agent(handle as u64, |agent| {
                    agent
                        .needs_resource(&asset, &resource, modified_ms)
                        .map_err(|error| error.to_string())
                })
            })
        });
        result.unwrap_or(true) as jboolean
    }

    #[no_mangle]
    pub extern "system" fn Java_com_example_photobackup_NativeBridge_nativeEnqueue(
        mut env: JNIEnv,
        _class: JClass,
        handle: jlong,
        input: JString,
    ) -> jstring {
        let result = java_string(&mut env, input).and_then(|json| {
            let parsed: EnqueueResource =
                serde_json::from_str(&json).map_err(|error| error.to_string())?;
            with_agent(handle as u64, |agent| {
                agent
                    .enqueue(parsed)
                    .map(Value::String)
                    .map_err(|error| error.to_string())
            })
        });
        to_jstring(&mut env, envelope(result))
    }

    #[no_mangle]
    pub extern "system" fn Java_com_example_photobackup_NativeBridge_nativeNext(
        mut env: JNIEnv,
        _class: JClass,
        handle: jlong,
        staging_root: JString,
    ) -> jstring {
        let result = java_string(&mut env, staging_root).and_then(|root| {
            with_agent(handle as u64, |agent| {
                agent
                    .next_prepared(root)
                    .and_then(|value| serde_json::to_value(value).map_err(Into::into))
                    .map_err(|error| error.to_string())
            })
        });
        to_jstring(&mut env, envelope(result))
    }

    fn two_string_operation(
        env: &mut JNIEnv,
        first: JString,
        second: JString,
        operation: impl FnOnce(String, String) -> Result<Value, String>,
    ) -> jstring {
        let result = java_string(env, first)
            .and_then(|first| java_string(env, second).and_then(|second| operation(first, second)));
        to_jstring(env, envelope(result))
    }

    #[no_mangle]
    pub extern "system" fn Java_com_example_photobackup_NativeBridge_nativeMarkUpload(
        mut env: JNIEnv,
        _class: JClass,
        handle: jlong,
        job: JString,
        upload: JString,
    ) -> jstring {
        two_string_operation(&mut env, job, upload, |job, upload| {
            with_agent(handle as u64, |agent| {
                agent
                    .mark_upload(&job, &upload)
                    .map(|_| Value::Null)
                    .map_err(|error| error.to_string())
            })
        })
    }

    #[no_mangle]
    pub extern "system" fn Java_com_example_photobackup_NativeBridge_nativeMarkPart(
        mut env: JNIEnv,
        _class: JClass,
        handle: jlong,
        job: JString,
        index: jint,
    ) -> jstring {
        let result = java_string(&mut env, job).and_then(|job| {
            with_agent(handle as u64, |agent| {
                agent
                    .mark_part_uploaded(&job, index as u32)
                    .map(|_| Value::Null)
                    .map_err(|error| error.to_string())
            })
        });
        to_jstring(&mut env, envelope(result))
    }

    #[no_mangle]
    pub extern "system" fn Java_com_example_photobackup_NativeBridge_nativeMarkComplete(
        mut env: JNIEnv,
        _class: JClass,
        handle: jlong,
        job: JString,
    ) -> jstring {
        let result = java_string(&mut env, job).and_then(|job| {
            with_agent(handle as u64, |agent| {
                agent
                    .mark_complete(&job)
                    .map(|_| Value::Null)
                    .map_err(|error| error.to_string())
            })
        });
        to_jstring(&mut env, envelope(result))
    }

    #[no_mangle]
    pub extern "system" fn Java_com_example_photobackup_NativeBridge_nativeMarkFailed(
        mut env: JNIEnv,
        _class: JClass,
        handle: jlong,
        job: JString,
        message: JString,
        retryable: jboolean,
    ) -> jstring {
        two_string_operation(&mut env, job, message, |job, message| {
            with_agent(handle as u64, |agent| {
                agent
                    .mark_failed(&job, &message, retryable != 0)
                    .map(|_| Value::Null)
                    .map_err(|error| error.to_string())
            })
        })
    }

    #[no_mangle]
    pub extern "system" fn Java_com_example_photobackup_NativeBridge_nativeStats(
        mut env: JNIEnv,
        _class: JClass,
        handle: jlong,
    ) -> jstring {
        let result = with_agent(handle as u64, |agent| {
            agent
                .stats()
                .and_then(|value| serde_json::to_value(value).map_err(Into::into))
                .map_err(|error| error.to_string())
        });
        to_jstring(&mut env, envelope(result))
    }
}
