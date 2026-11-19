//! Login intent, OS approval and legacy migration are separate states. Native operations run
//! only on the app's main thread; policy tests use a backend without touching login settings.

use anyhow::{
    Context,
    Result,
    bail,
};
use objc2::rc::Retained;
use objc2::runtime::{
    AnyClass,
    AnyObject,
};
use objc2::{
    MainThreadMarker,
    msg_send,
};
use objc2_foundation::{
    NSAppleEventDescriptor,
    NSAppleEventManager,
    NSError,
};

pub const SETTING: &str = "app.launchOnStartup";
const APPLIED: &str = "desktop.loginItemAppliedIntent";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    NotRegistered,
    Enabled,
    RequiresApproval,
    NotFound,
}

trait Backend {
    fn status(&self) -> Result<Status>;
    fn set_enabled(&mut self, enabled: bool) -> Result<()>;
    fn remove_legacy(&mut self) -> Result<()>;
}

fn reconcile(
    backend: &mut impl Backend,
    previous: Option<bool>,
    requested: bool,
    mut save: impl FnMut(bool) -> Result<()>,
) -> Result<Status> {
    let mut status = backend.status()?;
    // Only changed intent can register again. Disabling an item in System Settings must survive
    // app restarts, unrelated preference changes, and repeated filesystem notifications.
    if previous != Some(requested) {
        let already_applied = if requested {
            matches!(status, Status::Enabled | Status::RequiresApproval)
        } else {
            matches!(status, Status::NotRegistered | Status::NotFound)
        };
        if !already_applied {
            backend.set_enabled(requested)?;
            status = backend.status()?;
        }
        let confirmed = if requested {
            matches!(status, Status::Enabled | Status::RequiresApproval)
        } else {
            matches!(status, Status::NotRegistered | Status::NotFound)
        };
        if !confirmed {
            bail!("login item transition is not confirmed: {status:?}");
        }
        save(requested)?;
    }
    // Persist intent before cleanup: a cleanup retry must not overwrite a later OS-side denial.
    backend.remove_legacy()?;
    Ok(status)
}

#[link(name = "ServiceManagement", kind = "framework")]
unsafe extern "C" {}

struct Native {
    service: Retained<AnyObject>,
    _main_thread: MainThreadMarker,
}

impl Native {
    fn new(main_thread: MainThreadMarker) -> Result<Self> {
        let class = AnyClass::get(c"SMAppService").context("login items require macOS 13 or later")?;
        // SAFETY: the linked public SMAppService class provides this retained-object getter.
        let service: Option<Retained<AnyObject>> = unsafe { msg_send![class, mainAppService] };
        Ok(Self {
            service: service.context("main app login service unavailable")?,
            _main_thread: main_thread,
        })
    }
}

impl Backend for Native {
    fn status(&self) -> Result<Status> {
        // SAFETY: SMAppService.status returns the NSInteger enum defined in SMAppService.h.
        let value: isize = unsafe { msg_send![&*self.service, status] };
        match value {
            0 => Ok(Status::NotRegistered),
            1 => Ok(Status::Enabled),
            2 => Ok(Status::RequiresApproval),
            3 => Ok(Status::NotFound),
            _ => bail!("unknown login item status {value}"),
        }
    }

    fn set_enabled(&mut self, enabled: bool) -> Result<()> {
        // SAFETY: both public methods return BOOL and an autoreleased NSError out-parameter;
        // objc2's Result encoding retains the error. The main-app service does not stop this app.
        let result: Result<(), Retained<NSError>> = unsafe {
            if enabled {
                msg_send![&*self.service, registerAndReturnError: _]
            } else {
                msg_send![&*self.service, unregisterAndReturnError: _]
            }
        };
        result.map_err(|error| anyhow::anyhow!("login item request failed: {}", error.localizedDescription()))
    }

    fn remove_legacy(&mut self) -> Result<()> {
        let path = fig_util::directories::home_dir()?
            .join("Library/LaunchAgents")
            .join(format!("{}.plist", fig_util::consts::LAUNCH_AGENT_LABEL));
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
        // Do not bootout/unload the legacy job: it may own this running desktop. It has no
        // KeepAlive and its removed plist cannot launch it at the next login.
    }
}

pub fn apply() {
    let result = (|| -> Result<Status> {
        let main_thread = MainThreadMarker::new().context("login item reconciliation requires the main thread")?;
        // A development executable must not register a different main bundle or retire the
        // installed app's legacy launcher.
        require_installed_bundle()?;
        let mut backend = Native::new(main_thread)?;
        let previous = fig_settings::state::get_bool(APPLIED)?;
        let requested = fig_settings::settings::get_bool_or(SETTING, true);
        reconcile(&mut backend, previous, requested, |intent| {
            fig_settings::state::set_value(APPLIED, intent).map_err(Into::into)
        })
    })();
    match result {
        Ok(status) => tracing::info!(?status, "login item reconciled; approval is managed by macOS"),
        Err(error) => tracing::warn!(%error, "login item reconciliation deferred"),
    }
}

fn require_installed_bundle() -> Result<()> {
    let bundle = macos_utils::bundle::get_bundle_path().context("login items require an app bundle")?;
    if bundle != fig_integrations::cli_link::installed_bundle() {
        bail!("only the installed application may manage login items");
    }
    Ok(())
}

pub fn inspect(action: fig_proto::fig::InstallAction) -> fig_desktop_api::requests::RequestResult {
    use fig_desktop_api::requests::install::{
        installation_status,
        integration_result,
    };
    use fig_proto::fig::InstallAction;
    use fig_proto::fig::install_response::InstallationStatus;
    require_installed_bundle().map_err(|e| e.to_string())?;
    let marker = MainThreadMarker::new().ok_or("Login item request requires the main thread")?;
    let service = Native::new(marker).map_err(|e| e.to_string())?;
    Ok(Box::new(match action {
        InstallAction::Status => installation_status(match service.status().map_err(|e| e.to_string())? {
            Status::Enabled => InstallationStatus::Installed,
            Status::RequiresApproval => InstallationStatus::RequiresApproval,
            Status::NotRegistered | Status::NotFound => InstallationStatus::NotInstalled,
        }),
        InstallAction::Install => {
            let class = AnyClass::get(c"SMAppService").ok_or("Login Items unavailable")?;
            // SAFETY: SMAppService exposes this void class method on macOS 13 and later.
            let _: () = unsafe { msg_send![class, openSystemSettingsLoginItems] };
            integration_result(Ok::<(), &str>(()))
        },
        InstallAction::Uninstall => integration_result(Err("Turn off Start with macOS in Preferences.")),
    }))
}

pub fn unregister() -> Result<()> {
    require_installed_bundle()?;
    let main_thread = MainThreadMarker::new().context("login item removal requires the main thread")?;
    let mut backend = Native::new(main_thread)?;
    reconcile(&mut backend, None, false, |_| Ok(()))?;
    Ok(())
}

/// LaunchServices delivers the launch Apple Event when Tao enters its initial event-loop turn.
/// Delay the dashboard decision until then; SMAppService cannot supply --no-dashboard arguments.
pub fn launched_at_login() -> bool {
    if MainThreadMarker::new().is_none() {
        return false;
    }
    let Some(event) = NSAppleEventManager::sharedAppleEventManager().currentAppleEvent() else {
        return false;
    };
    is_login_event(&event)
}

fn is_login_event(event: &NSAppleEventDescriptor) -> bool {
    // SAFETY: public getter takes the AERegistry.h u32 keyword and returns an optional retained
    // descriptor. The generated binding enables unrelated CoreServices/Security features for
    // this integer alias; keep that dependency outside the desktop's framework surface.
    let marker: Option<Retained<NSAppleEventDescriptor>> =
        unsafe { msg_send![event, attributeDescriptorForKeyword: u32::from_be_bytes(*b"lgit")] };
    marker.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fake {
        status: Status,
        calls: Vec<&'static str>,
        fail: bool,
        cleanup_fail: bool,
    }
    impl Backend for Fake {
        fn status(&self) -> Result<Status> {
            Ok(self.status)
        }

        fn set_enabled(&mut self, enabled: bool) -> Result<()> {
            self.calls.push(if enabled { "register" } else { "unregister" });
            if self.fail {
                bail!("injected registration failure");
            }
            self.status = if enabled {
                Status::RequiresApproval
            } else {
                Status::NotRegistered
            };
            Ok(())
        }

        fn remove_legacy(&mut self) -> Result<()> {
            self.calls.push("cleanup");
            if self.cleanup_fail {
                bail!("injected cleanup failure");
            }
            Ok(())
        }
    }
    fn fake(status: Status) -> Fake {
        Fake {
            status,
            calls: vec![],
            fail: false,
            cleanup_fail: false,
        }
    }

    #[test]
    fn migration_waits_for_os_request_before_retiring_legacy() {
        let mut backend = fake(Status::NotRegistered);
        let mut saved = None;
        assert_eq!(
            reconcile(&mut backend, None, true, |value| {
                saved = Some(value);
                Ok(())
            })
            .unwrap(),
            Status::RequiresApproval
        );
        assert_eq!(saved, Some(true));
        assert_eq!(backend.calls, ["register", "cleanup"]);
    }
    #[test]
    fn os_denial_and_duplicate_notifications_do_not_register_again() {
        for status in [Status::NotRegistered, Status::RequiresApproval, Status::NotFound] {
            let mut backend = fake(status);
            reconcile(&mut backend, Some(true), true, |_| panic!("intent did not change")).unwrap();
            assert_eq!(backend.calls, ["cleanup"]);
        }
    }
    #[test]
    fn failed_registration_or_persistence_keeps_legacy_and_intent_retryable() {
        let mut backend = fake(Status::NotRegistered);
        backend.fail = true;
        assert!(reconcile(&mut backend, None, true, |_| panic!("must not mark migration complete")).is_err());
        assert_eq!(backend.calls, ["register"]);
        let mut backend = fake(Status::Enabled);
        assert!(reconcile(&mut backend, None, true, |_| bail!("disk full")).is_err());
        assert!(backend.calls.is_empty());
    }
    #[test]
    fn cleanup_failure_does_not_repeat_registration_after_os_denial() {
        let mut backend = fake(Status::NotRegistered);
        backend.cleanup_fail = true;
        let mut saved = None;
        assert!(
            reconcile(&mut backend, None, true, |value| {
                saved = Some(value);
                Ok(())
            })
            .is_err()
        );
        backend.status = Status::NotRegistered;
        backend.cleanup_fail = false;
        backend.calls.clear();
        reconcile(&mut backend, saved, true, |_| panic!("already saved")).unwrap();
        assert_eq!(backend.calls, ["cleanup"]);
    }
    #[test]
    fn explicit_disable_unregisters_pending_approval() {
        let mut backend = fake(Status::RequiresApproval);
        reconcile(&mut backend, Some(true), false, |value| {
            assert!(!value);
            Ok(())
        })
        .unwrap();
        assert_eq!(backend.calls, ["unregister", "cleanup"]);
    }
    #[test]
    fn launch_event_marker_distinguishes_login_from_normal_open() {
        use objc2::ClassType;
        // SAFETY: public constructor, matching AERegistry's u32/u32/object/i16/i32 ABI.
        let event: Retained<NSAppleEventDescriptor> = unsafe {
            msg_send![
                NSAppleEventDescriptor::class(),
                appleEventWithEventClass: u32::from_be_bytes(*b"aevt"),
                eventID: u32::from_be_bytes(*b"oapp"),
                targetDescriptor: None::<&NSAppleEventDescriptor>,
                returnID: -1_i16,
                transactionID: 0_i32
            ]
        };
        assert!(!is_login_event(&event));
        let marker = NSAppleEventDescriptor::descriptorWithBoolean(1);
        // SAFETY: public descriptor setter takes an object and a 32-bit AEKeyword.
        let _: () =
            unsafe { msg_send![&*event, setAttributeDescriptor: &*marker, forKeyword: u32::from_be_bytes(*b"lgit")] };
        assert!(is_login_event(&event));
    }
}
