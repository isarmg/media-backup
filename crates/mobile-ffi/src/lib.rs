//! Current product ABI: Foundation revision 2, length-delimited inputs and owned results.
#![allow(clippy::missing_safety_doc)]
use media_backup_agent_core::{
    Agent, AgentConfig, EnqueueResource, MOBILE_APPLICATION_VERSION, MOBILE_DATABASE_FILENAME,
    MOBILE_PRODUCT, MOBILE_REVISION, MOBILE_STAGING_DIRECTORY, MOBILE_STATE_EPOCH,
};
use sarmg_mobile_ffi::{self as ffi, FfiError, Handle, HandleRegistry, Payload, SarmgFfiResultV2};
use serde_json::{json, Value};
use std::{
    path::{Component, Path},
    sync::{Arc, OnceLock},
};

const MAX_PATH_BYTES: usize = 4096;
const MAX_IDENTIFIER_BYTES: usize = 4096;
static AGENTS: OnceLock<HandleRegistry<Arc<Agent>>> = OnceLock::new();

fn agents() -> &'static HandleRegistry<Arc<Agent>> {
    AGENTS.get_or_init(HandleRegistry::default)
}
fn internal(_: impl std::fmt::Display) -> FfiError {
    FfiError::internal()
}

fn require_path(path: &str, filename: &str) -> Result<(), FfiError> {
    let path = Path::new(path);
    if !path.is_absolute()
        || path.file_name().and_then(|v| v.to_str()) != Some(filename)
        || path.as_os_str().as_encoded_bytes().contains(&0)
        || path.components().any(|c| matches!(c, Component::ParentDir))
    {
        return Err(FfiError::invalid_argument());
    }
    Ok(())
}
fn open_impl(path: &str, config: &str) -> Result<u64, FfiError> {
    let config: AgentConfig =
        serde_json::from_str(config).map_err(|_| FfiError::invalid_argument())?;
    require_path(path, MOBILE_DATABASE_FILENAME)?;
    let agent = Agent::open(path, config).map_err(|error| match error {
        media_backup_agent_core::AgentError::InvalidContract(_) => FfiError::invalid_argument(),
        _ => FfiError::internal(),
    })?;
    agents().insert(Arc::new(agent)).map(Handle::to_u64)
}
fn close_impl(handle: u64) -> Result<(), FfiError> {
    agents().remove(Handle::from_u64(handle)).map(|_| ())
}
fn with_agent<T>(
    handle: u64,
    operation: impl FnOnce(&Agent) -> Result<T, FfiError>,
) -> Result<T, FfiError> {
    let agent = agents().get(Handle::from_u64(handle))?;
    operation(&agent)
}
fn enqueue_impl(handle: u64, input: &str) -> Result<Value, FfiError> {
    let input: EnqueueResource =
        serde_json::from_str(input).map_err(|_| FfiError::invalid_argument())?;
    with_agent(handle, |a| {
        a.enqueue(input).map(Value::String).map_err(internal)
    })
}
fn next_impl(handle: u64, staging: &str) -> Result<Value, FfiError> {
    require_path(staging, MOBILE_STAGING_DIRECTORY)?;
    with_agent(handle, |a| {
        serde_json::to_value(a.next_prepared(staging).map_err(internal)?).map_err(internal)
    })
}
fn stats_impl(handle: u64) -> Result<Value, FfiError> {
    with_agent(handle, |a| {
        serde_json::to_value(a.stats().map_err(internal)?).map_err(internal)
    })
}
fn envelope(value: Value) -> Result<String, FfiError> {
    let value = json!({
        "product": MOBILE_PRODUCT, "application_version": MOBILE_APPLICATION_VERSION,
        "revision": MOBILE_REVISION, "state_epoch": MOBILE_STATE_EPOCH,
        "ok": true, "value": value, "error": null,
    });
    let mut output = ffi::OutputBuffer::default();
    serde_json::to_writer(&mut output, &value).map_err(|_| FfiError::resource_exhausted())?;
    String::from_utf8(output.into_bytes()).map_err(internal)
}
fn json_payload(value: Value) -> Result<Payload, FfiError> {
    Payload::bytes(envelope(value)?.into_bytes())
}

#[no_mangle]
pub extern "C" fn mb_ffi_abi_revision() -> u32 {
    ffi::boundary(|| Ok(ffi::ABI_REVISION)).unwrap_or(0)
}
#[no_mangle]
pub unsafe extern "C" fn mb_open_v2(
    path: *const u8,
    path_len: usize,
    config: *const u8,
    config_len: usize,
    output: *mut SarmgFfiResultV2,
) -> i32 {
    unsafe {
        ffi::guard(output, || {
            let path = ffi::checked_utf8(path, path_len, MAX_PATH_BYTES)?;
            let config = ffi::checked_utf8(config, config_len, ffi::MAX_INPUT_BYTES)?;
            open_impl(path, config).map(Payload::value)
        })
    }
}
#[no_mangle]
pub unsafe extern "C" fn mb_close_v2(handle: u64, output: *mut SarmgFfiResultV2) -> i32 {
    unsafe {
        ffi::guard(output, || {
            close_impl(handle)?;
            Ok(Payload::default())
        })
    }
}
#[no_mangle]
pub unsafe extern "C" fn mb_needs_v2(
    handle: u64,
    asset: *const u8,
    asset_len: usize,
    resource: *const u8,
    resource_len: usize,
    modified_ms: i64,
    output: *mut SarmgFfiResultV2,
) -> i32 {
    unsafe {
        ffi::guard(output, || {
            let asset = ffi::checked_utf8(asset, asset_len, MAX_IDENTIFIER_BYTES)?;
            let resource = ffi::checked_utf8(resource, resource_len, MAX_IDENTIFIER_BYTES)?;
            with_agent(handle, |a| {
                a.needs_resource(asset, resource, modified_ms)
                    .map_err(internal)
            })
            .map(|needed| Payload::value(u64::from(needed)))
        })
    }
}
#[no_mangle]
pub unsafe extern "C" fn mb_enqueue_v2(
    handle: u64,
    input: *const u8,
    input_len: usize,
    output: *mut SarmgFfiResultV2,
) -> i32 {
    unsafe {
        ffi::guard(output, || {
            json_payload(enqueue_impl(
                handle,
                ffi::checked_utf8(input, input_len, ffi::MAX_INPUT_BYTES)?,
            )?)
        })
    }
}
#[no_mangle]
pub unsafe extern "C" fn mb_next_v2(
    handle: u64,
    staging: *const u8,
    staging_len: usize,
    output: *mut SarmgFfiResultV2,
) -> i32 {
    unsafe {
        ffi::guard(output, || {
            json_payload(next_impl(
                handle,
                ffi::checked_utf8(staging, staging_len, MAX_PATH_BYTES)?,
            )?)
        })
    }
}
#[no_mangle]
pub unsafe extern "C" fn mb_mark_upload_v2(
    handle: u64,
    job: *const u8,
    job_len: usize,
    upload: *const u8,
    upload_len: usize,
    output: *mut SarmgFfiResultV2,
) -> i32 {
    unsafe {
        ffi::guard(output, || {
            let job = ffi::checked_utf8(job, job_len, MAX_IDENTIFIER_BYTES)?;
            let upload = ffi::checked_utf8(upload, upload_len, MAX_IDENTIFIER_BYTES)?;
            with_agent(handle, |a| a.mark_upload(job, upload).map_err(internal))?;
            json_payload(Value::Null)
        })
    }
}
#[no_mangle]
pub unsafe extern "C" fn mb_mark_part_v2(
    handle: u64,
    job: *const u8,
    job_len: usize,
    index: u32,
    output: *mut SarmgFfiResultV2,
) -> i32 {
    unsafe {
        ffi::guard(output, || {
            let job = ffi::checked_utf8(job, job_len, MAX_IDENTIFIER_BYTES)?;
            with_agent(handle, |a| {
                a.mark_part_uploaded(job, index).map_err(internal)
            })?;
            json_payload(Value::Null)
        })
    }
}
#[no_mangle]
pub unsafe extern "C" fn mb_mark_complete_v2(
    handle: u64,
    job: *const u8,
    job_len: usize,
    output: *mut SarmgFfiResultV2,
) -> i32 {
    unsafe {
        ffi::guard(output, || {
            let job = ffi::checked_utf8(job, job_len, MAX_IDENTIFIER_BYTES)?;
            with_agent(handle, |a| a.mark_complete(job).map_err(internal))?;
            json_payload(Value::Null)
        })
    }
}
#[no_mangle]
pub unsafe extern "C" fn mb_mark_failed_v2(
    handle: u64,
    job: *const u8,
    job_len: usize,
    message: *const u8,
    message_len: usize,
    retryable: u8,
    output: *mut SarmgFfiResultV2,
) -> i32 {
    unsafe {
        ffi::guard(output, || {
            if retryable > 1 {
                return Err(FfiError::invalid_argument());
            }
            let job = ffi::checked_utf8(job, job_len, MAX_IDENTIFIER_BYTES)?;
            let message = ffi::checked_utf8(message, message_len, ffi::MAX_INPUT_BYTES)?;
            with_agent(handle, |a| {
                a.mark_failed(job, message, retryable == 1)
                    .map_err(internal)
            })?;
            json_payload(Value::Null)
        })
    }
}
#[no_mangle]
pub unsafe extern "C" fn mb_stats_v2(handle: u64, output: *mut SarmgFfiResultV2) -> i32 {
    unsafe { ffi::guard(output, || json_payload(stats_impl(handle)?)) }
}

#[cfg(any(target_os = "android", test, feature = "jni-host-tests"))]
mod android;

#[cfg(test)]
mod tests {
    use super::*;
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
    fn current_payload_rejects_unknown_missing_and_wrong_identity_without_queue_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(MOBILE_DATABASE_FILENAME);
        let handle = open_impl(path.to_str().unwrap(), &config()).unwrap();
        let mut unknown = current_enqueue_json();
        unknown["unknown_secret"] = json!("private credential");
        assert!(enqueue_impl(handle, &unknown.to_string()).is_err());
        for field in [
            "product",
            "application_version",
            "revision",
            "state_epoch",
            "remove_source_after_prepare",
        ] {
            let mut missing = current_enqueue_json();
            missing.as_object_mut().unwrap().remove(field);
            assert!(enqueue_impl(handle, &missing.to_string()).is_err());
            let mut wrong = current_enqueue_json();
            wrong[field] = json!("invalid-current-value");
            assert!(enqueue_impl(handle, &wrong.to_string()).is_err());
        }
        assert_eq!(stats_impl(handle).unwrap()["discovered"], 0);
        close_impl(handle).unwrap();
    }

    fn config() -> String {
        json!({
        "product": MOBILE_PRODUCT, "application_version": MOBILE_APPLICATION_VERSION,
        "revision": MOBILE_REVISION, "state_epoch": MOBILE_STATE_EPOCH, "part_size": 16 * 1024 * 1024,
    }).to_string()
    }
    #[test]
    fn current_abi_opens_reports_errors_and_rejects_repeated_close() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(MOBILE_DATABASE_FILENAME);
        let path = path.to_str().unwrap().as_bytes();
        let config = config();
        let mut out = SarmgFfiResultV2::default();
        unsafe {
            assert_eq!(
                mb_open_v2(
                    path.as_ptr(),
                    path.len(),
                    config.as_ptr(),
                    config.len(),
                    &mut out
                ),
                ffi::SARMG_FFI_OK
            );
            let first = out.value;
            assert_ne!(first, 0);
            ffi::sarmg_ffi_result_free_v2(&mut out);
            assert_eq!(mb_stats_v2(first, &mut out), ffi::SARMG_FFI_OK);
            let value: Value = serde_json::from_slice(
                ffi::checked_input(out.bytes.data, out.bytes.length, ffi::MAX_INPUT_BYTES).unwrap(),
            )
            .unwrap();
            assert_eq!(value["application_version"], MOBILE_APPLICATION_VERSION);
            ffi::sarmg_ffi_result_free_v2(&mut out);
            assert_eq!(mb_close_v2(first, &mut out), ffi::SARMG_FFI_OK);
            assert_eq!(mb_close_v2(first, &mut out), ffi::SARMG_FFI_INVALID_HANDLE);
            ffi::sarmg_ffi_result_free_v2(&mut out);
            assert_eq!(
                mb_open_v2(
                    path.as_ptr(),
                    path.len(),
                    config.as_ptr(),
                    config.len(),
                    &mut out
                ),
                ffi::SARMG_FFI_OK
            );
            let second = out.value;
            assert_ne!(first, second);
            assert_eq!(mb_stats_v2(first, &mut out), ffi::SARMG_FFI_INVALID_HANDLE);
            ffi::sarmg_ffi_result_free_v2(&mut out);
            assert_eq!(mb_close_v2(second, &mut out), ffi::SARMG_FFI_OK);
        }
    }
    #[test]
    fn invalid_input_and_identity_are_rejected_without_filesystem_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent").join(MOBILE_DATABASE_FILENAME);
        let path = path.to_str().unwrap().as_bytes();
        let mut out = SarmgFfiResultV2::default();
        unsafe {
            for part_size in [0, 64 * 1024 * 1024 + 1, u64::MAX] {
                let mut value: Value = serde_json::from_str(&config()).unwrap();
                value["part_size"] = json!(part_size);
                let input = value.to_string();
                assert_eq!(
                    mb_open_v2(
                        path.as_ptr(),
                        path.len(),
                        input.as_ptr(),
                        input.len(),
                        &mut out
                    ),
                    ffi::SARMG_FFI_INVALID_ARGUMENT
                );
                ffi::sarmg_ffi_result_free_v2(&mut out);
            }
            assert_eq!(
                mb_open_v2(path.as_ptr(), path.len(), std::ptr::null(), 1, &mut out),
                ffi::SARMG_FFI_INVALID_ARGUMENT
            );
            ffi::sarmg_ffi_result_free_v2(&mut out);
            assert_eq!(
                mb_open_v2(path.as_ptr(), path.len(), [255].as_ptr(), 1, &mut out),
                ffi::SARMG_FFI_INVALID_ARGUMENT
            );
            ffi::sarmg_ffi_result_free_v2(&mut out);
            for field in [
                "product",
                "application_version",
                "revision",
                "state_epoch",
                "part_size",
            ] {
                let mut value: Value = serde_json::from_str(&config()).unwrap();
                value.as_object_mut().unwrap().remove(field);
                let input = value.to_string();
                assert_ne!(
                    mb_open_v2(
                        path.as_ptr(),
                        path.len(),
                        input.as_ptr(),
                        input.len(),
                        &mut out
                    ),
                    ffi::SARMG_FFI_OK
                );
                ffi::sarmg_ffi_result_free_v2(&mut out);
            }
        }
        assert!(!dir.path().join("absent").exists());
    }
}
