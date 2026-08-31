#![allow(clippy::missing_safety_doc)]

use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::{CStr, CString},
    os::raw::c_char,
    path::{Component, Path},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
};

use media_backup_agent_core::{
    Agent, AgentConfig, EnqueueResource, MOBILE_APPLICATION_VERSION, MOBILE_DATABASE_FILENAME,
    MOBILE_PRODUCT, MOBILE_REVISION, MOBILE_STAGING_DIRECTORY, MOBILE_STATE_EPOCH,
};
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
    require_epoch_path(database_path, MOBILE_DATABASE_FILENAME, "database")?;
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

fn enqueue_impl(handle: u64, input: &str) -> Result<Value, String> {
    let parsed: EnqueueResource = serde_json::from_str(input).map_err(|error| error.to_string())?;
    with_agent(handle, |agent| {
        agent
            .enqueue(parsed)
            .map(Value::String)
            .map_err(|error| error.to_string())
    })
}

fn next_impl(handle: u64, staging_root: &str) -> Result<Value, String> {
    require_epoch_path(staging_root, MOBILE_STAGING_DIRECTORY, "staging")?;
    with_agent(handle, |agent| {
        agent
            .next_prepared(staging_root)
            .and_then(|value| serde_json::to_value(value).map_err(Into::into))
            .map_err(|error| error.to_string())
    })
}

fn envelope(result: Result<Value, String>) -> String {
    match result {
        Ok(value) => json!({
            "product": MOBILE_PRODUCT,
            "application_version": MOBILE_APPLICATION_VERSION,
            "revision": MOBILE_REVISION,
            "state_epoch": MOBILE_STATE_EPOCH,
            "ok": true,
            "value": value,
            "error": null,
        })
        .to_string(),
        Err(error) => json!({
            "product": MOBILE_PRODUCT,
            "application_version": MOBILE_APPLICATION_VERSION,
            "revision": MOBILE_REVISION,
            "state_epoch": MOBILE_STATE_EPOCH,
            "ok": false,
            "value": null,
            "error": error,
        })
        .to_string(),
    }
}

fn require_epoch_path(path: &str, filename: &str, purpose: &str) -> Result<(), String> {
    let path = Path::new(path);
    if !path.is_absolute()
        || path.file_name().and_then(|value| value.to_str()) != Some(filename)
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!(
            "mobile v0.2 {purpose} path must be absolute and end in {filename}"
        ));
    }
    Ok(())
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
pub unsafe extern "C" fn mb_v0_2_r1_open(
    database_path: *const c_char,
    config_json: *const c_char,
) -> u64 {
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
pub extern "C" fn mb_v0_2_r1_close(handle: u64) {
    if let Ok(mut registry) = agents().lock() {
        registry.remove(&handle);
    }
}

#[no_mangle]
pub unsafe extern "C" fn mb_v0_2_r1_needs(
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
pub unsafe extern "C" fn mb_v0_2_r1_enqueue(handle: u64, input_json: *const c_char) -> *mut c_char {
    let result = read_c_string(input_json).and_then(|input| enqueue_impl(handle, &input));
    c_string(envelope(result))
}

#[no_mangle]
pub unsafe extern "C" fn mb_v0_2_r1_next(handle: u64, staging_root: *const c_char) -> *mut c_char {
    let result = read_c_string(staging_root).and_then(|root| next_impl(handle, &root));
    c_string(envelope(result))
}

#[no_mangle]
pub unsafe extern "C" fn mb_v0_2_r1_mark_upload(
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
pub unsafe extern "C" fn mb_v0_2_r1_mark_part(
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
pub unsafe extern "C" fn mb_v0_2_r1_mark_complete(
    handle: u64,
    job_id: *const c_char,
) -> *mut c_char {
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
pub unsafe extern "C" fn mb_v0_2_r1_mark_failed(
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
pub extern "C" fn mb_v0_2_r1_stats(handle: u64) -> *mut c_char {
    let result = with_agent(handle, |agent| {
        agent
            .stats()
            .and_then(|value| serde_json::to_value(value).map_err(Into::into))
            .map_err(|error| error.to_string())
    });
    c_string(envelope(result))
}

#[no_mangle]
pub extern "C" fn mb_v0_2_r1_last_error() -> *const c_char {
    LAST_ERROR.with(|value| value.borrow().as_ptr())
}

#[no_mangle]
pub unsafe extern "C" fn mb_v0_2_r1_string_free(value: *mut c_char) {
    if !value.is_null() {
        drop(CString::from_raw(value));
    }
}

#[cfg(any(target_os = "android", test))]
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
    pub extern "system" fn Java_com_example_mediabackup_v02_NativeBridgeV02_openV02R1(
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
    pub extern "system" fn Java_com_example_mediabackup_v02_NativeBridgeV02_closeV02R1(
        _env: JNIEnv,
        _class: JClass,
        handle: jlong,
    ) {
        mb_v0_2_r1_close(handle as u64);
    }

    #[no_mangle]
    pub extern "system" fn Java_com_example_mediabackup_v02_NativeBridgeV02_needsV02R1(
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
    pub extern "system" fn Java_com_example_mediabackup_v02_NativeBridgeV02_enqueueV02R1(
        mut env: JNIEnv,
        _class: JClass,
        handle: jlong,
        input: JString,
    ) -> jstring {
        let result =
            java_string(&mut env, input).and_then(|json| enqueue_impl(handle as u64, &json));
        to_jstring(&mut env, envelope(result))
    }

    #[no_mangle]
    pub extern "system" fn Java_com_example_mediabackup_v02_NativeBridgeV02_nextV02R1(
        mut env: JNIEnv,
        _class: JClass,
        handle: jlong,
        staging_root: JString,
    ) -> jstring {
        let result =
            java_string(&mut env, staging_root).and_then(|root| next_impl(handle as u64, &root));
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
    pub extern "system" fn Java_com_example_mediabackup_v02_NativeBridgeV02_markUploadV02R1(
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
    pub extern "system" fn Java_com_example_mediabackup_v02_NativeBridgeV02_markPartV02R1(
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
    pub extern "system" fn Java_com_example_mediabackup_v02_NativeBridgeV02_markCompleteV02R1(
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
    pub extern "system" fn Java_com_example_mediabackup_v02_NativeBridgeV02_markFailedV02R1(
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
    pub extern "system" fn Java_com_example_mediabackup_v02_NativeBridgeV02_statsV02R1(
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

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;

    static NEXT_SANDBOX: AtomicU64 = AtomicU64::new(1);

    struct TestSandbox(PathBuf);

    impl TestSandbox {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "photo-mobile-v02-{}-{}",
                std::process::id(),
                NEXT_SANDBOX.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestSandbox {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn current_config_json() -> String {
        json!({
            "product": MOBILE_PRODUCT,
            "application_version": MOBILE_APPLICATION_VERSION,
            "revision": MOBILE_REVISION,
            "state_epoch": MOBILE_STATE_EPOCH,
            "part_size": 16 * 1024 * 1024,
        })
        .to_string()
    }

    fn current_enqueue_json() -> Value {
        json!({
            "product": MOBILE_PRODUCT,
            "application_version": MOBILE_APPLICATION_VERSION,
            "revision": MOBILE_REVISION,
            "state_epoch": MOBILE_STATE_EPOCH,
            "source_asset_id": "asset",
            "source_resource_id": "resource",
            "media_kind": "photo",
            "role": "primary",
            "file_path": "/source-is-not-opened-by-enqueue",
            "filename": "photo.jpg",
            "mime_type": "image/jpeg",
            "source_created_at_ms": 1,
            "modified_ms": 2,
            "source_size": 3,
            "metadata_json": null,
            "remove_source_after_prepare": false,
        })
    }

    #[test]
    fn complete_v01_config_is_rejected_before_any_filesystem_write() {
        let container = TestSandbox::new();
        let absent_sandbox = container.path().join("v02-sandbox-must-remain-absent");
        let database = absent_sandbox.join(MOBILE_DATABASE_FILENAME);
        let v01_config = json!({
            "master_key_b64": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
            "dedupe_key_b64": "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB=",
            "part_size": 16 * 1024 * 1024,
        });

        let database_c = CString::new(database.to_string_lossy().as_bytes()).unwrap();
        let v01_config_c = CString::new(v01_config.to_string()).unwrap();
        assert_eq!(
            unsafe { mb_v0_2_r1_open(database_c.as_ptr(), v01_config_c.as_ptr()) },
            0
        );
        let error = LAST_ERROR.with(|value| value.borrow().to_string_lossy().into_owned());
        assert!(error.contains("unknown field") || error.contains("missing field"));
        assert!(!absent_sandbox.exists());

        let missing_part_size = json!({
            "product": MOBILE_PRODUCT,
            "application_version": MOBILE_APPLICATION_VERSION,
            "revision": MOBILE_REVISION,
            "state_epoch": MOBILE_STATE_EPOCH,
        });
        assert!(open_impl(&database.to_string_lossy(), &missing_part_size.to_string()).is_err());
        assert!(!absent_sandbox.exists());

        for (field, wrong) in [
            ("product", json!("another-product")),
            ("application_version", json!("0.1.0")),
            ("revision", json!(2)),
            ("state_epoch", json!("media-backup-mobile-v0.1")),
            ("part_size", json!(0)),
        ] {
            let mut config: Value = serde_json::from_str(&current_config_json()).unwrap();
            config[field] = wrong;
            assert!(
                open_impl(&database.to_string_lossy(), &config.to_string()).is_err(),
                "invalid {field} was accepted"
            );
            assert!(!absent_sandbox.exists());
        }
    }

    #[test]
    fn v02_sandbox_never_reads_writes_or_deletes_v01_state() {
        let sandbox = TestSandbox::new();
        let legacy_database = sandbox.path().join("agent.sqlite");
        let legacy_staging = sandbox.path().join("backup-staging");
        let legacy_marker = legacy_staging.join("encrypted-v01-part");
        let legacy_database_bytes = b"v0.1 sqlite generation";
        let legacy_staging_bytes = b"v0.1 staging generation";
        fs::write(&legacy_database, legacy_database_bytes).unwrap();
        fs::create_dir(&legacy_staging).unwrap();
        fs::write(&legacy_marker, legacy_staging_bytes).unwrap();

        let current_database = sandbox.path().join(MOBILE_DATABASE_FILENAME);
        let current_staging = sandbox.path().join(MOBILE_STAGING_DIRECTORY);
        let handle =
            open_impl(&current_database.to_string_lossy(), &current_config_json()).unwrap();
        fs::create_dir(&current_staging).unwrap();
        assert_eq!(
            next_impl(handle, &current_staging.to_string_lossy()).unwrap(),
            Value::Null
        );

        assert!(open_impl(&legacy_database.to_string_lossy(), &current_config_json()).is_err());
        assert!(next_impl(handle, &legacy_staging.to_string_lossy()).is_err());
        assert_eq!(fs::read(&legacy_database).unwrap(), legacy_database_bytes);
        assert_eq!(fs::read(&legacy_marker).unwrap(), legacy_staging_bytes);
        mb_v0_2_r1_close(handle);
    }

    #[test]
    fn ffi_json_rejects_unknown_missing_and_wrong_identity_without_queue_writes() {
        let sandbox = TestSandbox::new();
        let database = sandbox.path().join(MOBILE_DATABASE_FILENAME);
        let handle = open_impl(&database.to_string_lossy(), &current_config_json()).unwrap();

        let mut unknown = current_enqueue_json();
        unknown["master_key_b64"] = Value::String("legacy-secret".to_owned());
        assert!(enqueue_impl(handle, &unknown.to_string()).is_err());

        for (field, wrong) in [
            ("product", json!("another-product")),
            ("application_version", json!("0.1.0")),
            ("revision", json!(2)),
            ("state_epoch", json!("media-backup-mobile-v0.1")),
        ] {
            let mut input = current_enqueue_json();
            input[field] = wrong;
            assert!(
                enqueue_impl(handle, &input.to_string()).is_err(),
                "invalid {field} was accepted"
            );
        }

        let mut missing = current_enqueue_json();
        missing
            .as_object_mut()
            .unwrap()
            .remove("remove_source_after_prepare");
        assert!(enqueue_impl(handle, &missing.to_string()).is_err());

        let stats = with_agent(handle, |agent| {
            agent.stats().map_err(|error| error.to_string())
        })
        .unwrap();
        assert_eq!(stats.discovered, 0);
        mb_v0_2_r1_close(handle);
    }
}
