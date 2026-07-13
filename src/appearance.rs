//! OS light/dark appearance detection, for the follow-system theme mode.
//!
//! - **macOS**: reads the global `AppleInterfaceStyle` preference (the same value
//!   `defaults read -g AppleInterfaceStyle` returns) in-process via CoreFoundation —
//!   no subprocess, mirroring the raw-FFI style in [`crate::media`].
//! - **Linux**: reads the XDG Desktop Portal `org.freedesktop.appearance /
//!   color-scheme` setting over D-Bus (GNOME/KDE/…). A watcher thread
//!   ([`start_watcher`]) does all the D-Bus I/O off the UI/audio threads and
//!   publishes the result into a shared atomic, so [`detect`] stays a cheap,
//!   non-blocking read on every platform.
//! - **Everything else**: reports `None`, so the caller keeps its configured dark
//!   theme as the fallback.
//!
//! `detect` never blocks and is safe to call from the UI tick.

/// The system's light/dark appearance.
///
/// Constructed by [`detect`] on macOS + Linux; other platforms always return `None`,
/// so the variants are dead code there (mirrors the platform-gated allow on the
/// neutral types in [`crate::media`]). Tests construct them on all platforms.
#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Appearance {
    Light,
    Dark,
}

/// Read the current OS appearance, or `None` when the platform has no notion of it
/// (or it can't be read). Cheap and synchronous — safe to poll from the UI tick.
#[cfg(target_os = "macos")]
pub fn detect() -> Option<Appearance> {
    macos::detect()
}

/// Linux: read the value the D-Bus watcher last published (see [`start_watcher`]).
#[cfg(target_os = "linux")]
pub fn detect() -> Option<Appearance> {
    linux::detect()
}

/// Other platforms: no supported system-appearance source yet.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn detect() -> Option<Appearance> {
    None
}

/// Start any background watcher the platform needs so [`detect`] returns live values.
/// Linux spawns a D-Bus watcher thread; macOS reads on demand (no watcher needed), so
/// this is a no-op there and elsewhere. Call once at startup.
pub fn start_watcher() {
    #[cfg(target_os = "linux")]
    linux::start_watcher();
}

#[cfg(target_os = "macos")]
mod macos {
    use super::Appearance;
    use std::ffi::c_void;

    type CFTypeRef = *const c_void;
    type CFStringRef = *const c_void;
    type CFAllocatorRef = *const c_void;
    type Boolean = u8;
    type CFIndex = isize;
    type CFStringEncoding = u32;

    const K_CF_STRING_ENCODING_UTF8: CFStringEncoding = 0x0800_0100;
    const KEY: &[u8] = b"AppleInterfaceStyle";

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFPreferencesAnyApplication: CFStringRef;
        static kCFPreferencesCurrentUser: CFStringRef;
        static kCFPreferencesAnyHost: CFStringRef;
        fn CFPreferencesAppSynchronize(application_id: CFStringRef) -> Boolean;
        fn CFPreferencesCopyValue(
            key: CFStringRef,
            application_id: CFStringRef,
            user_name: CFStringRef,
            host_name: CFStringRef,
        ) -> CFTypeRef;
        fn CFStringCreateWithBytes(
            alloc: CFAllocatorRef,
            bytes: *const u8,
            num_bytes: CFIndex,
            encoding: CFStringEncoding,
            is_external_representation: Boolean,
        ) -> CFStringRef;
        fn CFRelease(cf: CFTypeRef);
    }

    /// Read `AppleInterfaceStyle` from the global preferences domain. The key holds
    /// a `"Dark"` string in Dark mode and is absent (NULL) in Light mode — including
    /// under macOS "Auto", where it reflects the *current* effective style. So its
    /// mere presence means Dark.
    pub fn detect() -> Option<Appearance> {
        // SAFETY: the domain/user/host arguments are framework-owned immortal
        // CFStrings; `key` is a CFString we create and release here. Both this call
        // and CFPreferences are thread-safe, so it's safe on the UI thread.
        // `CFPreferencesCopyValue` returns either NULL or a +1-retained CFType we
        // release. No Rust-owned pointers cross the FFI boundary.
        unsafe {
            let key = CFStringCreateWithBytes(
                std::ptr::null(),
                KEY.as_ptr(),
                KEY.len() as CFIndex,
                K_CF_STRING_ENCODING_UTF8,
                0,
            );
            if key.is_null() {
                return None;
            }
            // flush this process's cached copy so a live toggle is seen promptly
            CFPreferencesAppSynchronize(kCFPreferencesAnyApplication);
            let value = CFPreferencesCopyValue(
                key,
                kCFPreferencesAnyApplication,
                kCFPreferencesCurrentUser,
                kCFPreferencesAnyHost,
            );
            CFRelease(key);
            let dark = !value.is_null();
            if dark {
                CFRelease(value);
            }
            Some(if dark {
                Appearance::Dark
            } else {
                Appearance::Light
            })
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::Appearance;
    use std::sync::atomic::{AtomicU8, Ordering};
    use zbus::blocking::{Connection, Proxy};
    use zbus::zvariant::{OwnedValue, Value};

    /// The color-scheme the watcher last read, so [`detect`] never touches D-Bus on
    /// the UI thread: 0 = unknown (→ fall back to the dark theme), 1 = light, 2 = dark.
    static SCHEME: AtomicU8 = AtomicU8::new(0);

    const DEST: &str = "org.freedesktop.portal.Desktop";
    const PATH: &str = "/org/freedesktop/portal/desktop";
    const IFACE: &str = "org.freedesktop.portal.Settings";
    const NS: &str = "org.freedesktop.appearance";
    const KEY: &str = "color-scheme";

    pub fn detect() -> Option<Appearance> {
        match SCHEME.load(Ordering::Relaxed) {
            1 => Some(Appearance::Light),
            2 => Some(Appearance::Dark),
            _ => None,
        }
    }

    /// Store the portal's color-scheme (0 = no preference, 1 = prefer dark, 2 = prefer
    /// light) as our own code (0 = unknown, 1 = light, 2 = dark). "No preference" stays
    /// unknown so the caller keeps its dark-theme fallback.
    fn store_scheme(portal: u32) {
        let code = match portal {
            1 => 2, // prefer dark
            2 => 1, // prefer light
            _ => 0, // no preference / unknown
        };
        SCHEME.store(code, Ordering::Relaxed);
    }

    /// The portal wraps the value in a variant, sometimes doubly (`v[v[u]]`), so strip
    /// any number of nested `Value::Value` layers down to the `u32`.
    fn variant_to_u32(v: &Value<'_>) -> Option<u32> {
        match v {
            Value::U32(n) => Some(*n),
            Value::Value(inner) => variant_to_u32(inner),
            _ => None,
        }
    }

    /// Spawn the D-Bus watcher once. It seeds the current scheme, then blocks on the
    /// portal's `SettingChanged` signal, updating [`SCHEME`] on every change — all off
    /// the UI/audio threads. Exits quietly when there's no session bus or portal (the
    /// scheme stays "unknown", so lyrfin just keeps the configured dark theme).
    pub fn start_watcher() {
        let _ = std::thread::Builder::new()
            .name("lyrfin-appearance".into())
            .spawn(|| {
                if let Err(e) = watch() {
                    log::debug!(target: "lyrfin::appearance", "appearance watcher stopped: {e}");
                }
            });
    }

    fn watch() -> zbus::Result<()> {
        let conn = Connection::session()?;
        let proxy = Proxy::new(&conn, DEST, PATH, IFACE)?;
        // Seed the current value (this also activates the portal name on the bus, which
        // `receive_signal` needs). A missing key just leaves the scheme unknown.
        let seed: zbus::Result<OwnedValue> = proxy.call("Read", &(NS, KEY));
        if let Ok(v) = seed
            && let Some(n) = variant_to_u32(&v)
        {
            store_scheme(n);
        }
        for signal in proxy.receive_signal("SettingChanged")? {
            let (namespace, key, value): (String, String, OwnedValue) = signal.body()?;
            if namespace == NS
                && key == KEY
                && let Some(n) = variant_to_u32(&value)
            {
                store_scheme(n);
            }
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn variant_unwraps_any_nesting_depth() {
            // the portal returns the value single- or double-wrapped in `v`
            assert_eq!(variant_to_u32(&Value::U32(2)), Some(2));
            assert_eq!(
                variant_to_u32(&Value::Value(Box::new(Value::U32(1)))),
                Some(1)
            );
            assert_eq!(
                variant_to_u32(&Value::Value(Box::new(Value::Value(Box::new(Value::U32(
                    2
                )))))),
                Some(2)
            );
            assert_eq!(variant_to_u32(&Value::Bool(true)), None, "non-u32 → None");
        }

        #[test]
        fn scheme_mapping_follows_the_portal_spec() {
            // portal: 0 = no preference, 1 = prefer dark, 2 = prefer light
            store_scheme(1);
            assert_eq!(detect(), Some(Appearance::Dark));
            store_scheme(2);
            assert_eq!(detect(), Some(Appearance::Light));
            store_scheme(0);
            assert_eq!(detect(), None, "no preference → dark-theme fallback");
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    /// The CoreFoundation read must agree with the canonical
    /// `defaults read -g AppleInterfaceStyle` (present `"Dark"` ⇒ Dark, absent ⇒
    /// Light) — verifies the FFI binding reads the right domain/key on the real OS.
    #[test]
    fn detect_agrees_with_system_defaults() {
        let out = std::process::Command::new("defaults")
            .args(["read", "-g", "AppleInterfaceStyle"])
            .output()
            .expect("run `defaults`");
        let defaults_dark = out.status.success()
            && String::from_utf8_lossy(&out.stdout)
                .trim()
                .eq_ignore_ascii_case("dark");
        let got = super::detect().expect("macOS detection returns Some");
        assert_eq!(
            got == super::Appearance::Dark,
            defaults_dark,
            "detect() must match `defaults` on the current appearance (got {got:?})"
        );
    }
}
