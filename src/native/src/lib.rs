//! Portal-backed JNI adapter. Original implementation; no vendor native code.
#![deny(unsafe_op_in_unsafe_fn)]

pub mod files;
pub mod portal;
mod usb;

use jni::errors::{Error as JniError, ErrorPolicy};
use jni::objects::{JByteArray, JCharArray, JClass, JIntArray, JObjectArray, JString};
use jni::strings::JNIString;
use jni::sys::{jboolean, jint, jlong};
use jni::{Env, EnvUnowned, jni_sig, jni_str};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

static PORTAL: Mutex<Option<Arc<portal::Portal>>> = Mutex::new(None);
static USB: OnceLock<Mutex<usb::Controller>> = OnceLock::new();

pub(crate) fn portal() -> Result<Arc<portal::Portal>, portal::PortalError> {
    let mut state = PORTAL
        .lock()
        .map_err(|_| portal::PortalError::Failed("Portal state is poisoned".into()))?;
    if let Some(portal) = state.as_ref() {
        return Ok(Arc::clone(portal));
    }
    let portal = Arc::new(portal::Portal::connect()?);
    *state = Some(Arc::clone(&portal));
    Ok(portal)
}

fn controller() -> Result<MutexGuard<'static, usb::Controller>, BridgeError> {
    USB.get_or_init(|| Mutex::new(usb::Controller::new()))
        .lock()
        .map_err(|_| BridgeError::Usb(usb::Error::Usb("USB state is poisoned".into())))
}

#[derive(Debug)]
enum BridgeError {
    Jni(JniError),
    Usb(usb::Error),
    File(files::FileError),
    Invalid(String),
    Io(String),
    Panic(String),
}

impl From<JniError> for BridgeError {
    fn from(e: JniError) -> Self {
        Self::Jni(e)
    }
}
impl From<usb::Error> for BridgeError {
    fn from(e: usb::Error) -> Self {
        Self::Usb(e)
    }
}
impl From<files::FileError> for BridgeError {
    fn from(e: files::FileError) -> Self {
        Self::File(e)
    }
}

impl BridgeError {
    fn class(&self) -> &'static str {
        match self {
            Self::Usb(usb::Error::Cancelled) => {
                "com/owon/uppersoft/vds/core/usb/CDevice$AccessCancelled"
            }
            Self::Usb(usb::Error::InvalidArgument(_)) | Self::Invalid(_) => {
                "java/lang/IllegalArgumentException"
            }
            Self::Usb(_) => "ch/ntb/usb/USBException",
            Self::File(_) | Self::Io(_) => "java/io/IOException",
            Self::Jni(_) | Self::Panic(_) => "java/lang/RuntimeException",
        }
    }
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Jni(e) => e.fmt(f),
            Self::Usb(e) => e.fmt(f),
            Self::File(e) => e.fmt(f),
            Self::Invalid(e) | Self::Io(e) | Self::Panic(e) => e.fmt(f),
        }
    }
}

// JNI entry-point pattern and panic containment follow jni-rs 0.22.4:
// https://github.com/jni-rs/jni-rs/blob/5ae9458a4ec44c5318f37ddc7569c1d4ae8a69e7/crates/jni/src/env.rs
struct JavaErrors;
impl<T: Default> ErrorPolicy<T, BridgeError> for JavaErrors {
    type Captures<'local: 'method, 'method> = ();
    fn on_error<'local: 'method, 'method>(
        env: &mut Env<'local>,
        _: &mut (),
        error: BridgeError,
    ) -> jni::errors::Result<T> {
        if !env.exception_check() {
            match env.throw_new(
                JNIString::new(error.class()),
                JNIString::new(error.to_string()),
            ) {
                Ok(()) | Err(JniError::JavaException) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(T::default())
    }
    fn on_panic<'local: 'method, 'method>(
        env: &mut Env<'local>,
        cap: &mut (),
        payload: Box<dyn std::any::Any + Send>,
    ) -> jni::errors::Result<T> {
        let message = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or("Native adapter panicked");
        Self::on_error(env, cap, BridgeError::Panic(message.into()))
    }
    fn on_internal_jni_error<'local: 'method, 'method>(_: &mut (), _: JniError) -> T {
        std::process::abort()
    }
    fn on_internal_panic<'local: 'method, 'method>(
        _: &mut (),
        _: Box<dyn std::any::Any + Send>,
    ) -> T {
        std::process::abort()
    }
}

fn strict_string(env: &mut Env<'_>, value: &JString<'_>) -> Result<String, BridgeError> {
    if value.is_null() {
        return Ok(String::new());
    }
    // Java permits unpaired surrogates. Validate UTF-16 rather than replacing them.
    let object = env
        .call_method(value, jni_str!("toCharArray"), jni_sig!("()[C"), &[])?
        .l()?;
    let chars = JCharArray::cast_local(env, object)?;
    let mut units = vec![0; chars.len(env)?];
    chars.get_region(env, 0, &mut units)?;
    decode_utf16(&units).map_err(BridgeError::Invalid)
}

fn decode_utf16(units: &[u16]) -> Result<String, String> {
    if units.contains(&0) {
        return Err("Portal strings must not contain NUL".into());
    }
    String::from_utf16(units).map_err(|_| "Portal strings must contain valid UTF-16".into())
}

fn file_string(env: &mut Env<'_>, value: &JString<'_>) -> Result<String, BridgeError> {
    strict_string(env, value).map_err(|e| match e {
        BridgeError::Invalid(s) => BridgeError::Io(s),
        other => other,
    })
}

fn string_array<'local>(
    env: &mut Env<'local>,
    values: &[String],
) -> Result<JObjectArray<'local, JString<'local>>, BridgeError> {
    let result = JObjectArray::<JString>::new(env, values.len(), JString::null())?;
    for (i, value) in values.iter().enumerate() {
        let text = JString::from_str(env, value)?;
        result.set_element(env, i, &text)?;
    }
    Ok(result)
}

// SAFETY for the ten exports: symbol names and signatures match the native
// declarations in CDevice.java and FilePortal.java. Only the JVM calls them.
// EnvUnowned::with_env contains panics and keeps local references on this thread.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_owon_uppersoft_vds_core_usb_CDevice_initNative<'local>(
    mut env: EnvUnowned<'local>,
    _: JClass<'local>,
) {
    env.with_env(|_| -> Result<(), BridgeError> {
        controller()?.init()?;
        Ok(())
    })
    .resolve::<JavaErrors>()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_owon_uppersoft_vds_core_usb_CDevice_releaseNative<'local>(
    mut env: EnvUnowned<'local>,
    _: JClass<'local>,
) {
    env.with_env(|_| -> Result<(), BridgeError> {
        controller()?.shutdown()?;
        Ok(())
    })
    .resolve::<JavaErrors>()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_owon_uppersoft_vds_core_usb_CDevice_enumerateNative<'local>(
    mut env: EnvUnowned<'local>,
    _: JClass<'local>,
) -> JObjectArray<'local, JString<'local>> {
    env.with_env(|env| -> Result<_, BridgeError> { string_array(env, &controller()?.enumerate()?) })
        .resolve::<JavaErrors>()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_owon_uppersoft_vds_core_usb_CDevice_openNative<'local>(
    mut env: EnvUnowned<'local>,
    _: JClass<'local>,
    id: JString<'local>,
    metadata: JIntArray<'local>,
) -> jlong {
    env.with_env(|env| -> Result<_, BridgeError> {
        if id.is_null() || metadata.is_null() || metadata.len(env)? != 8 {
            return Err(BridgeError::Invalid(
                "Invalid scope ID or metadata buffer".into(),
            ));
        }
        let id = strict_string(env, &id)?;
        let mut usb = controller()?;
        let (token, description) = usb.open(&id)?;
        if let Err(error) = metadata.set_region(env, 0, &description) {
            usb.close(token)?;
            return Err(error.into());
        }
        Ok(token)
    })
    .resolve::<JavaErrors>()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_owon_uppersoft_vds_core_usb_CDevice_closeNative<'local>(
    mut env: EnvUnowned<'local>,
    _: JClass<'local>,
    token: jlong,
) {
    env.with_env(|_| -> Result<(), BridgeError> {
        controller()?.close(token)?;
        Ok(())
    })
    .resolve::<JavaErrors>()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_owon_uppersoft_vds_core_usb_CDevice_closeProbeNative<'local>(
    mut env: EnvUnowned<'local>,
    _: JClass<'local>,
    token: jlong,
) {
    env.with_env(|_| -> Result<(), BridgeError> {
        controller()?.close_probe(token)?;
        Ok(())
    })
    .resolve::<JavaErrors>()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_owon_uppersoft_vds_core_usb_CDevice_forgetNative<'local>(
    mut env: EnvUnowned<'local>,
    _: JClass<'local>,
    id: JString<'local>,
) {
    env.with_env(|env| -> Result<(), BridgeError> {
        controller()?.forget(&strict_string(env, &id)?)?;
        Ok(())
    })
    .resolve::<JavaErrors>()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_owon_uppersoft_vds_core_usb_CDevice_resetNative<'local>(
    mut env: EnvUnowned<'local>,
    _: JClass<'local>,
    token: jlong,
) {
    env.with_env(|_| -> Result<(), BridgeError> {
        controller()?.reset(token)?;
        Ok(())
    })
    .resolve::<JavaErrors>()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_owon_uppersoft_vds_core_usb_CDevice_transferNative<'local>(
    mut env: EnvUnowned<'local>,
    _: JClass<'local>,
    token: jlong,
    data: JByteArray<'local>,
    size: jint,
    timeout: jint,
    read: jboolean,
) -> jint {
    env.with_env(|env| -> Result<_, BridgeError> {
        if data.is_null() || size <= 0 || size as usize > data.len(env)? || timeout <= 0 {
            return Err(BridgeError::Invalid(
                "Invalid USB transfer buffer, size or timeout".into(),
            ));
        }
        let mut java_bytes = vec![0i8; size as usize];
        if !read {
            data.get_region(env, 0, &mut java_bytes)?;
        }
        let mut bytes: Vec<u8> = java_bytes.iter().map(|&b| b as u8).collect();
        let transferred = controller()?.transfer(token, &mut bytes, timeout, read)?;
        assert!(
            transferred <= bytes.len(),
            "USB transport exceeded buffer length"
        );
        if read {
            for (out, &input) in java_bytes.iter_mut().zip(&bytes).take(transferred) {
                *out = input as i8;
            }
            data.set_region(env, 0, &java_bytes[..transferred])?;
        }
        Ok(transferred as jint)
    })
    .resolve::<JavaErrors>()
}

#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_vds1022_portal_FilePortal_chooseNative<'local>(
    mut env: EnvUnowned<'local>,
    _: JClass<'local>,
    save: jboolean,
    title: JString<'local>,
    name: JString<'local>,
    folder: JString<'local>,
    labels: JObjectArray<'local, JString<'local>>,
    patterns: JObjectArray<'local, JObjectArray<'local, JString<'local>>>,
    selected_filter: jint,
    multiple: jboolean,
    directory: jboolean,
    accept: JString<'local>,
) -> JObjectArray<'local, JString<'local>> {
    env.with_env(|env| -> Result<_, BridgeError> {
        if labels.is_null()
            || patterns.is_null()
            || labels.len(env)? != patterns.len(env)?
            || selected_filter < 0
        {
            return Err(BridgeError::Io("Invalid file filters".into()));
        }
        let mut filters = Vec::with_capacity(labels.len(env)?);
        for i in 0..labels.len(env)? {
            let java_label = labels.get_element(env, i)?;
            let label = file_string(env, &java_label)?;
            let array = patterns.get_element(env, i)?;
            if array.is_null() {
                return Err(BridgeError::Io("Missing filter patterns".into()));
            }
            let mut globs = Vec::with_capacity(array.len(env)?);
            for j in 0..array.len(env)? {
                let glob = array.get_element(env, j)?;
                globs.push(file_string(env, &glob)?);
            }
            filters.push(files::Filter::new(label, globs)?);
        }
        let request = files::FileRequest::new(
            save,
            file_string(env, &title)?,
            file_string(env, &name)?,
            file_string(env, &folder)?,
            file_string(env, &accept)?,
            filters,
            selected_filter as usize,
            multiple,
            directory,
        )?;
        let portal = portal().map_err(|e| BridgeError::Io(e.to_string()))?;
        match files::choose(&portal, request)? {
            None => Ok(JObjectArray::<JString>::null()),
            Some(selection) => {
                let mut result = vec![selection.selected_filter.to_string()];
                result.extend(selection.uris);
                string_array(env, &result)
            }
        }
    })
    .resolve::<JavaErrors>()
}

#[cfg(test)]
mod tests {
    use super::decode_utf16;
    #[test]
    fn portal_strings_require_valid_unicode_without_nul() {
        assert_eq!(
            decode_utf16(&"señal 💡".encode_utf16().collect::<Vec<_>>()).unwrap(),
            "señal 💡"
        );
        assert!(decode_utf16(&[0xd800]).is_err());
        assert!(decode_utf16(&[b'a' as u16, 0]).is_err());
    }
}
