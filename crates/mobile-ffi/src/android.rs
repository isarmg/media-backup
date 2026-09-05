use super::*;
use ::jni::{
    objects::{JClass, JString},
    sys::{jboolean, jint, jlong, jstring},
    JNIEnv,
};
use ffi::jni::{guard, new_string, read_string};

// Fault injection is compiled only for the host-JVM test artifact, never for
// the Android/iOS release commands, which do not enable jni-host-tests.
#[cfg(feature = "jni-host-tests")]
#[no_mangle]
pub extern "system" fn Java_org_sarmg_mediabackup_NativeBridgeV2_panicProbe(
    mut env: JNIEnv,
    _class: JClass,
) -> jint {
    guard(&mut env, 0, |_| panic!("private-secret-panic"))
}

#[no_mangle]
pub extern "system" fn Java_org_sarmg_mediabackup_NativeBridgeV2_abiRevision(
    mut env: JNIEnv,
    _class: JClass,
) -> jint {
    guard(&mut env, 0, |_| Ok(ffi::ABI_REVISION as jint))
}
#[no_mangle]
pub extern "system" fn Java_org_sarmg_mediabackup_NativeBridgeV2_open(
    mut env: JNIEnv,
    _class: JClass,
    path: JString,
    config: JString,
) -> jlong {
    guard(&mut env, 0, |env| {
        let path = read_string(env, path, MAX_PATH_BYTES)?;
        let config = read_string(env, config, ffi::MAX_INPUT_BYTES)?;
        open_impl(&path, &config).map(|h| h as jlong)
    })
}
#[no_mangle]
pub extern "system" fn Java_org_sarmg_mediabackup_NativeBridgeV2_close(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    guard(&mut env, (), |_| close_impl(handle as u64))
}
#[no_mangle]
pub extern "system" fn Java_org_sarmg_mediabackup_NativeBridgeV2_needs(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    asset: JString,
    resource: JString,
    modified_ms: jlong,
) -> jboolean {
    guard(&mut env, 0, |env| {
        let asset = read_string(env, asset, MAX_IDENTIFIER_BYTES)?;
        let resource = read_string(env, resource, MAX_IDENTIFIER_BYTES)?;
        with_agent(handle as u64, |a| {
            a.needs_resource(&asset, &resource, modified_ms)
                .map_err(internal)
        })
        .map(jboolean::from)
    })
}
#[no_mangle]
pub extern "system" fn Java_org_sarmg_mediabackup_NativeBridgeV2_enqueue(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    input: JString,
) -> jstring {
    guard(&mut env, std::ptr::null_mut(), |env| {
        let input = read_string(env, input, ffi::MAX_INPUT_BYTES)?;
        new_string(env, envelope(enqueue_impl(handle as u64, &input)?)?)
    })
}
#[no_mangle]
pub extern "system" fn Java_org_sarmg_mediabackup_NativeBridgeV2_next(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    staging: JString,
) -> jstring {
    guard(&mut env, std::ptr::null_mut(), |env| {
        let staging = read_string(env, staging, MAX_PATH_BYTES)?;
        new_string(env, envelope(next_impl(handle as u64, &staging)?)?)
    })
}
#[no_mangle]
pub extern "system" fn Java_org_sarmg_mediabackup_NativeBridgeV2_markUpload(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    job: JString,
    upload: JString,
) -> jstring {
    guard(&mut env, std::ptr::null_mut(), |env| {
        let job = read_string(env, job, MAX_IDENTIFIER_BYTES)?;
        let upload = read_string(env, upload, MAX_IDENTIFIER_BYTES)?;
        with_agent(handle as u64, |a| {
            a.mark_upload(&job, &upload).map_err(internal)
        })?;
        new_string(env, envelope(Value::Null)?)
    })
}
#[no_mangle]
pub extern "system" fn Java_org_sarmg_mediabackup_NativeBridgeV2_markPart(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    job: JString,
    index: jint,
) -> jstring {
    guard(&mut env, std::ptr::null_mut(), |env| {
        let index = u32::try_from(index).map_err(|_| FfiError::invalid_argument())?;
        let job = read_string(env, job, MAX_IDENTIFIER_BYTES)?;
        with_agent(handle as u64, |a| {
            a.mark_part_uploaded(&job, index).map_err(internal)
        })?;
        new_string(env, envelope(Value::Null)?)
    })
}
#[no_mangle]
pub extern "system" fn Java_org_sarmg_mediabackup_NativeBridgeV2_markComplete(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    job: JString,
) -> jstring {
    guard(&mut env, std::ptr::null_mut(), |env| {
        let job = read_string(env, job, MAX_IDENTIFIER_BYTES)?;
        with_agent(handle as u64, |a| a.mark_complete(&job).map_err(internal))?;
        new_string(env, envelope(Value::Null)?)
    })
}
#[no_mangle]
pub extern "system" fn Java_org_sarmg_mediabackup_NativeBridgeV2_markFailed(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    job: JString,
    message: JString,
    retryable: jboolean,
) -> jstring {
    guard(&mut env, std::ptr::null_mut(), |env| {
        if retryable > 1 {
            return Err(FfiError::invalid_argument());
        }
        let job = read_string(env, job, MAX_IDENTIFIER_BYTES)?;
        let message = read_string(env, message, ffi::MAX_INPUT_BYTES)?;
        with_agent(handle as u64, |a| {
            a.mark_failed(&job, &message, retryable == 1)
                .map_err(internal)
        })?;
        new_string(env, envelope(Value::Null)?)
    })
}
#[no_mangle]
pub extern "system" fn Java_org_sarmg_mediabackup_NativeBridgeV2_stats(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jstring {
    guard(&mut env, std::ptr::null_mut(), |env| {
        new_string(env, envelope(stats_impl(handle as u64)?)?)
    })
}
