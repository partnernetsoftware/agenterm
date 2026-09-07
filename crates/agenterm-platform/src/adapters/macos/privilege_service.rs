//! macOS SMAppService plus AuthorizationRight lifecycle mechanism.
//!
//! No call here opens System Settings or acquires an operation right. The only
//! mutations are the explicitly selected service registration and exact right
//! installation/removal operations invoked by the facade.

use std::{ffi::CString, ptr};

use core_foundation::{
    base::{CFType, CFTypeRef, TCFType},
    boolean::CFBoolean,
    dictionary::CFDictionary,
    number::CFNumber,
    string::CFString,
};
use objc2::{msg_send, msg_send_id, rc::Retained, runtime::AnyClass, runtime::AnyObject};
use objc2_foundation::{NSError, NSString};
use security_framework_sys::authorization;

use crate::privilege_service::{
    Backend, BackendError, PrivilegeDaemonStatus, PrivilegeRightStatus,
};

#[link(name = "ServiceManagement", kind = "framework")]
unsafe extern "C" {}

const STATUS_NOT_REGISTERED: isize = 0;
const STATUS_ENABLED: isize = 1;
const STATUS_REQUIRES_APPROVAL: isize = 2;
const STATUS_NOT_FOUND: isize = 3;

pub(super) struct MacosPrivilegeServiceBackend;

impl Backend for MacosPrivilegeServiceBackend {
    fn daemon_status(&mut self, plist_name: &str) -> Result<PrivilegeDaemonStatus, BackendError> {
        with_service(plist_name, |service| {
            // SAFETY: SMAppService.status is a nonmutating NSInteger property.
            let status: isize = unsafe { msg_send![service, status] };
            match status {
                STATUS_NOT_REGISTERED => Ok(PrivilegeDaemonStatus::NotRegistered),
                STATUS_ENABLED => Ok(PrivilegeDaemonStatus::Enabled),
                STATUS_REQUIRES_APPROVAL => Ok(PrivilegeDaemonStatus::RequiresApproval),
                STATUS_NOT_FOUND => Ok(PrivilegeDaemonStatus::NotFound),
                value => Err(BackendError(format!(
                    "SMAppService returned unknown status {value}"
                ))),
            }
        })
    }

    fn right_status(&mut self, right: &str) -> Result<PrivilegeRightStatus, BackendError> {
        right_status(right)
    }

    fn install_right(&mut self, right: &str, rule: &str) -> Result<(), BackendError> {
        install_right(right, rule)
    }

    fn remove_right(&mut self, right: &str) -> Result<(), BackendError> {
        remove_right(right)
    }

    fn register_daemon(&mut self, plist_name: &str) -> Result<(), BackendError> {
        mutate_service(plist_name, "registerAndReturnError:")
    }

    fn unregister_daemon(&mut self, plist_name: &str) -> Result<(), BackendError> {
        mutate_service(plist_name, "unregisterAndReturnError:")
    }
}

fn with_service<T>(
    plist_name: &str,
    operation: impl FnOnce(&AnyObject) -> Result<T, BackendError>,
) -> Result<T, BackendError> {
    objc2::rc::autoreleasepool(|_| {
        let class = AnyClass::get("SMAppService")
            .ok_or_else(|| BackendError("SMAppService is unavailable before macOS 13".into()))?;
        let plist = NSString::from_str(plist_name);
        // SAFETY: +daemonServiceWithPlistName: returns a retained-compatible
        // SMAppService object for a live NSString argument.
        let service: Retained<AnyObject> =
            unsafe { msg_send_id![class, daemonServiceWithPlistName: &*plist] };
        operation(&service)
    })
}

fn mutate_service(plist_name: &str, selector: &str) -> Result<(), BackendError> {
    with_service(plist_name, |service| {
        let mut error: Option<Retained<NSError>> = None;
        // SAFETY: both methods have the identical `BOOL (NSError **)` ABI.
        // The selector is chosen only from the two literals below.
        let succeeded: bool = match selector {
            "registerAndReturnError:" => unsafe {
                msg_send![service, registerAndReturnError: &mut error]
            },
            "unregisterAndReturnError:" => unsafe {
                msg_send![service, unregisterAndReturnError: &mut error]
            },
            _ => {
                return Err(BackendError(
                    "invalid SMAppService mutation selector".into(),
                ));
            }
        };
        if succeeded {
            return Ok(());
        }
        let message = error
            .as_deref()
            .map(|error| error.localizedDescription().to_string())
            .unwrap_or_else(|| "SMAppService returned false without NSError".into());
        Err(BackendError(message))
    })
}

fn right_status(right: &str) -> Result<PrivilegeRightStatus, BackendError> {
    let right =
        CString::new(right).map_err(|_| BackendError("authorization right contains NUL".into()))?;
    let mut definition = ptr::null();
    // SAFETY: right is a live NUL-terminated name and definition is an output
    // owned under Core Foundation's create rule on success.
    let status = unsafe { authorization::AuthorizationRightGet(right.as_ptr(), &mut definition) };
    if status == authorization::errAuthorizationDenied {
        return Ok(PrivilegeRightStatus::Missing);
    }
    if status != authorization::errAuthorizationSuccess {
        return Err(os_status("AuthorizationRightGet", status));
    }
    if definition.is_null() {
        return Err(BackendError(
            "AuthorizationRightGet succeeded without a definition".into(),
        ));
    }
    // SAFETY: AuthorizationRightGet returned this dictionary under the create
    // rule, so this owner releases it exactly once.
    let definition =
        unsafe { CFDictionary::<CFString, CFType>::wrap_under_create_rule(definition) };
    if expected_definition(&definition) {
        Ok(PrivilegeRightStatus::Expected)
    } else {
        Ok(PrivilegeRightStatus::Conflict)
    }
}

fn expected_definition(definition: &CFDictionary<CFString, CFType>) -> bool {
    let rule_key = CFString::new("rule");
    let shared_key = CFString::new("shared");
    let timeout_key = CFString::new("timeout");

    let Some(rule) = definition.find(&rule_key) else {
        return false;
    };
    let Some(shared) = definition.find(&shared_key) else {
        return false;
    };
    let Some(timeout) = definition.find(&timeout_key) else {
        return false;
    };

    cf_string(rule.as_CFTypeRef()).is_some_and(|value| value == "authenticate-admin")
        && cf_bool(shared.as_CFTypeRef()) == Some(false)
        && cf_i32(timeout.as_CFTypeRef()) == Some(0)
}

fn cf_string(value: CFTypeRef) -> Option<String> {
    if unsafe { core_foundation::base::CFGetTypeID(value) }
        != unsafe { core_foundation::string::CFStringGetTypeID() }
    {
        return None;
    }
    // SAFETY: the type id proves this is a CFString, and the source dictionary
    // keeps the borrowed value alive for this conversion.
    Some(unsafe { CFString::wrap_under_get_rule(value.cast()) }.to_string())
}

fn cf_bool(value: CFTypeRef) -> Option<bool> {
    if unsafe { core_foundation::base::CFGetTypeID(value) }
        != unsafe { core_foundation::boolean::CFBooleanGetTypeID() }
    {
        return None;
    }
    // SAFETY: the type id proves this is a CFBoolean and wrapping under the get
    // rule balances its temporary retain when the wrapper drops.
    Some(bool::from(unsafe {
        CFBoolean::wrap_under_get_rule(value.cast())
    }))
}

fn cf_i32(value: CFTypeRef) -> Option<i32> {
    if unsafe { core_foundation::base::CFGetTypeID(value) }
        != unsafe { core_foundation::number::CFNumberGetTypeID() }
    {
        return None;
    }
    // SAFETY: the type id proves this is a CFNumber and the dictionary retains
    // it through this conversion.
    unsafe { CFNumber::wrap_under_get_rule(value.cast()) }.to_i32()
}

fn install_right(right: &str, rule: &str) -> Result<(), BackendError> {
    let right =
        CString::new(right).map_err(|_| BackendError("authorization right contains NUL".into()))?;
    let mut authorization_ref = ptr::null_mut();
    // SAFETY: null rights/environment creates an empty noninteractive
    // AuthorizationRef. No operation right is acquired here.
    let create_status = unsafe {
        authorization::AuthorizationCreate(
            ptr::null(),
            ptr::null(),
            authorization::kAuthorizationFlagDefaults,
            &mut authorization_ref,
        )
    };
    if create_status != authorization::errAuthorizationSuccess || authorization_ref.is_null() {
        return Err(os_status("AuthorizationCreate", create_status));
    }
    let owner = AuthorizationOwner(authorization_ref);

    let rule_key = CFString::new("rule");
    let shared_key = CFString::new("shared");
    let timeout_key = CFString::new("timeout");
    let rule_value = CFString::new(rule);
    let shared_value = CFBoolean::false_value();
    let timeout_value = CFNumber::from(0_i32);
    let definition = CFDictionary::from_CFType_pairs(&[
        (rule_key, as_cf_type(&rule_value)),
        (shared_key, as_cf_type(&shared_value)),
        (timeout_key, as_cf_type(&timeout_value)),
    ]);
    // SAFETY: every reference is live for the call; null localization fields
    // request no UI description lookup.
    let status = unsafe {
        authorization::AuthorizationRightSet(
            owner.0,
            right.as_ptr(),
            definition.as_CFTypeRef(),
            ptr::null(),
            ptr::null_mut(),
            ptr::null(),
        )
    };
    if status == authorization::errAuthorizationSuccess {
        Ok(())
    } else {
        Err(os_status("AuthorizationRightSet", status))
    }
}

fn remove_right(right: &str) -> Result<(), BackendError> {
    if right_status(right)? != PrivilegeRightStatus::Expected {
        return Err(BackendError(
            "refusing to remove an absent or conflicting authorization right".into(),
        ));
    }
    let right =
        CString::new(right).map_err(|_| BackendError("authorization right contains NUL".into()))?;
    let mut authorization_ref = ptr::null_mut();
    // SAFETY: creates an empty noninteractive authorization handle used only
    // to remove the already verified exact right.
    let create_status = unsafe {
        authorization::AuthorizationCreate(
            ptr::null(),
            ptr::null(),
            authorization::kAuthorizationFlagDefaults,
            &mut authorization_ref,
        )
    };
    if create_status != authorization::errAuthorizationSuccess || authorization_ref.is_null() {
        return Err(os_status("AuthorizationCreate", create_status));
    }
    let owner = AuthorizationOwner(authorization_ref);
    // SAFETY: owner and the NUL-terminated exact right are live for the call.
    let status = unsafe { authorization::AuthorizationRightRemove(owner.0, right.as_ptr()) };
    if status == authorization::errAuthorizationSuccess {
        Ok(())
    } else {
        Err(os_status("AuthorizationRightRemove", status))
    }
}

fn as_cf_type(value: &impl TCFType) -> CFType {
    // SAFETY: the returned wrapper retains the live Core Foundation value.
    unsafe { CFType::wrap_under_get_rule(value.as_CFTypeRef()) }
}

struct AuthorizationOwner(authorization::AuthorizationRef);

impl Drop for AuthorizationOwner {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: this owner holds the sole live AuthorizationRef.
            let _ = unsafe {
                authorization::AuthorizationFree(
                    self.0,
                    authorization::kAuthorizationFlagDestroyRights,
                )
            };
            self.0 = ptr::null_mut();
        }
    }
}

fn os_status(operation: &str, status: i32) -> BackendError {
    BackendError(format!("{operation} failed with OSStatus {status}"))
}
