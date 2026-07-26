//! Making grans's keychain item readable without a password prompt.
//!
//! macOS ties a keychain item to the *code signature* of the application that
//! created it, not to its path. A cargo-built binary has no stable one: on
//! Apple Silicon the linker applies an ad-hoc signature, which carries no
//! signing identity, so the designated requirement degrades to a hash of those
//! exact bytes. Every rebuild is a different application as far as the keychain
//! is concerned, so the ACL written during `grans auth login` matches one build
//! and nothing after it, and "Always Allow" only pins whichever binary happens
//! to be running at the time.
//!
//! The item is therefore given the permissive ACL that
//! `security add-generic-password -A` writes: a trusted-application list of
//! NULL, which macOS reads as "any application, no prompt". The refresh token
//! stays encrypted at rest and out of readable backups, which is what the
//! keychain buys over the `0600` fallback file, but any process running as the
//! user can now read it without being challenged.

use std::ffi::c_void;
use std::ptr;

use anyhow::{bail, Result};
use core_foundation::base::TCFType;
use core_foundation::string::CFString;
use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
use core_foundation_sys::base::{CFRelease, CFTypeRef, OSStatus};
use core_foundation_sys::string::CFStringRef;
use security_framework::os::macos::access::SecAccess;
use security_framework::os::macos::keychain::SecKeychain;
use security_framework::os::macos::keychain_item::SecKeychainItem;
use security_framework_sys::base::{SecAccessRef, SecKeychainItemRef};

/// An entry in a `SecAccess`'s ACL list. Opaque; only handed back to Security.
type SecACLRef = *const c_void;

/// `SecKeychainPromptSelector`, a bitfield of the conditions that force a
/// prompt even for a trusted caller. Zero forces none of them.
type SecKeychainPromptSelector = u16;

// Neither `security-framework` nor its `-sys` crate binds these: the former
// declares the `SecAccess` type and nothing that builds one, and the latter
// covers only `SecAccessGetTypeID`.
unsafe extern "C" {
    fn SecAccessCreate(
        descriptor: CFStringRef,
        trustedlist: CFArrayRef,
        accessRef: *mut SecAccessRef,
    ) -> OSStatus;

    fn SecAccessCopyACLList(accessRef: SecAccessRef, aclList: *mut CFArrayRef) -> OSStatus;

    fn SecACLSetContents(
        acl: SecACLRef,
        applicationList: CFArrayRef,
        description: CFStringRef,
        promptSelector: SecKeychainPromptSelector,
    ) -> OSStatus;

    fn SecKeychainItemSetAccess(itemRef: SecKeychainItemRef, accessRef: SecAccessRef) -> OSStatus;

    fn SecKeychainFindGenericPassword(
        keychainOrArray: CFTypeRef,
        serviceNameLength: u32,
        serviceName: *const c_void,
        accountNameLength: u32,
        accountName: *const c_void,
        passwordLength: *mut u32,
        passwordData: *mut *mut c_void,
        itemRef: *mut SecKeychainItemRef,
    ) -> OSStatus;
}

/// Let every application read the item under `service`/`account` in the
/// default keychain without a password prompt.
pub fn allow_any_application(service: &str, account: &str) -> Result<()> {
    set_permissive_access(None, service, account)
}

/// The same, against a specific keychain. Tests pass a throwaway one rather
/// than touching the login keychain.
fn set_permissive_access(
    keychain: Option<&SecKeychain>,
    service: &str,
    account: &str,
) -> Result<()> {
    let item = find_item(keychain, service, account)?;
    let descriptor = CFString::new(service);
    let access = permissive_access(&descriptor)?;

    check(
        unsafe {
            SecKeychainItemSetAccess(item.as_concrete_TypeRef(), access.as_concrete_TypeRef())
        },
        "attach a permissive ACL to the keychain item",
    )
}

/// Locate the stored item without decrypting it.
///
/// `security-framework` only exposes a lookup that returns the secret too, and
/// reading it is its own authorization: on an entry still carrying the old ACL
/// that is a second password prompt for a value this has no use for.
fn find_item(
    keychain: Option<&SecKeychain>,
    service: &str,
    account: &str,
) -> Result<SecKeychainItem> {
    let mut item: SecKeychainItemRef = ptr::null_mut();

    check(
        unsafe {
            SecKeychainFindGenericPassword(
                // A null search list means the user's default keychains.
                keychain.map_or(ptr::null(), |keychain| keychain.as_CFTypeRef()),
                service.len() as u32,
                service.as_ptr().cast(),
                account.len() as u32,
                account.as_ptr().cast(),
                // Null out-params for the secret skip returning it at all.
                ptr::null_mut(),
                ptr::null_mut(),
                &mut item,
            )
        },
        "find grans's entry in the keychain",
    )?;

    Ok(unsafe { SecKeychainItem::wrap_under_create_rule(item) })
}

/// Build an access object that challenges nobody.
fn permissive_access(descriptor: &CFString) -> Result<SecAccess> {
    let mut access: SecAccessRef = ptr::null_mut();

    // A NULL trusted list here means "the calling application", which is the
    // default and the very thing being replaced; the ACLs it seeds are then
    // rewritten below.
    check(
        unsafe { SecAccessCreate(descriptor.as_concrete_TypeRef(), ptr::null(), &mut access) },
        "create a keychain access object",
    )?;

    let access = unsafe { SecAccess::wrap_under_create_rule(access) };
    clear_trusted_applications(&access, descriptor)?;
    Ok(access)
}

/// Drop every ACL's trusted-application list.
fn clear_trusted_applications(access: &SecAccess, descriptor: &CFString) -> Result<()> {
    let mut list: CFArrayRef = ptr::null();
    check(
        unsafe { SecAccessCopyACLList(access.as_concrete_TypeRef(), &mut list) },
        "read the ACLs of a keychain access object",
    )?;
    let list = OwnedArray(list);

    for acl in list.entries() {
        check(
            // NULL is what macOS reads as "any application, no prompt". An
            // empty array is the opposite: nobody is trusted, always ask.
            unsafe { SecACLSetContents(acl, ptr::null(), descriptor.as_concrete_TypeRef(), 0) },
            "clear the trusted-application list on an ACL",
        )?;
    }

    Ok(())
}

/// A `CFArrayRef` handed over under the create rule, released when dropped.
struct OwnedArray(CFArrayRef);

impl OwnedArray {
    fn entries(&self) -> Vec<SecACLRef> {
        let count = unsafe { CFArrayGetCount(self.0) };
        (0..count)
            .map(|index| unsafe { CFArrayGetValueAtIndex(self.0, index) })
            .collect()
    }
}

impl Drop for OwnedArray {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0.cast()) };
        }
    }
}

/// Turn a non-zero `OSStatus` into an error naming what was being attempted.
fn check(status: OSStatus, doing: &str) -> Result<()> {
    if status == 0 {
        return Ok(());
    }

    bail!("Failed to {doing} (OSStatus {status})")
}

#[cfg(test)]
mod tests {
    use super::*;
    use security_framework::os::macos::keychain::CreateOptions;
    use security_framework::os::macos::passwords::find_generic_password;
    use tempfile::TempDir;

    // Read-side counterparts of the calls above, needed only to assert on what
    // was written.
    unsafe extern "C" {
        fn SecKeychainItemCopyAccess(
            itemRef: SecKeychainItemRef,
            accessRef: *mut SecAccessRef,
        ) -> OSStatus;

        fn SecACLCopyContents(
            acl: SecACLRef,
            applicationList: *mut CFArrayRef,
            description: *mut CFStringRef,
            promptSelector: *mut SecKeychainPromptSelector,
        ) -> OSStatus;
    }

    const SERVICE: &str = "grans-acl-test";
    const ACCOUNT: &str = "granola-session";

    /// A throwaway keychain holding one secret, so the tests never touch the
    /// login keychain.
    fn keychain_with_secret(dir: &TempDir) -> SecKeychain {
        let keychain = CreateOptions::new()
            .password("test-password")
            .create(dir.path().join("grans-test.keychain"))
            .unwrap();
        keychain
            .set_generic_password(SERVICE, ACCOUNT, b"secret")
            .unwrap();
        keychain
    }

    /// For each ACL on the stored item, whether it trusts a fixed list of
    /// applications rather than all of them.
    fn acls_naming_applications(keychain: &SecKeychain) -> Vec<bool> {
        let item = find_item(Some(keychain), SERVICE, ACCOUNT).unwrap();

        let mut access: SecAccessRef = ptr::null_mut();
        let status = unsafe { SecKeychainItemCopyAccess(item.as_concrete_TypeRef(), &mut access) };
        assert_eq!(status, 0, "SecKeychainItemCopyAccess");
        let access = unsafe { SecAccess::wrap_under_create_rule(access) };

        let mut list: CFArrayRef = ptr::null();
        let status = unsafe { SecAccessCopyACLList(access.as_concrete_TypeRef(), &mut list) };
        assert_eq!(status, 0, "SecAccessCopyACLList");
        let list = OwnedArray(list);

        list.entries()
            .into_iter()
            .map(|acl| {
                let mut applications: CFArrayRef = ptr::null();
                let mut description: CFStringRef = ptr::null();
                let mut prompt: SecKeychainPromptSelector = 0;
                let status = unsafe {
                    SecACLCopyContents(acl, &mut applications, &mut description, &mut prompt)
                };
                assert_eq!(status, 0, "SecACLCopyContents");
                !applications.is_null()
            })
            .collect()
    }

    #[test]
    fn test_stored_item_starts_pinned_to_its_creator() {
        // The control for the test below, and the bug itself: left alone,
        // macOS pins the item to the binary that wrote it, whose ad-hoc
        // signature changes with every rebuild.
        let dir = TempDir::new().unwrap();
        let keychain = keychain_with_secret(&dir);

        assert!(acls_naming_applications(&keychain).contains(&true));
    }

    /// Rewriting an ACL blocks where no one can answer a prompt.
    ///
    /// Reading works headless, but `SecKeychainItemSetAccess` needs ChangeACL
    /// on an item pinned to a binary whose code signature a CI runner cannot
    /// match. Security escalates that to `SecurityAgent`, which has no window
    /// server to draw on and never returns, so the test hangs rather than
    /// failing. `cargo test` has no per-test timeout, so one of these holds the
    /// whole binary open until the job is killed.
    ///
    /// Run them with `--ignored` on a Mac with a desktop session, or through
    /// the macos-keychain-probe workflow, which times each one out.
    const HEADLESS: &str = "writes an ACL: hangs on a headless runner, see the module docs";

    #[test]
    #[ignore = "writes an ACL: hangs on a headless runner"]
    fn test_permissive_access_trusts_every_application() {
        let dir = TempDir::new().unwrap();
        let keychain = keychain_with_secret(&dir);

        set_permissive_access(Some(&keychain), SERVICE, ACCOUNT).expect(HEADLESS);

        assert!(!acls_naming_applications(&keychain).contains(&true));
    }

    #[test]
    #[ignore = "writes an ACL: hangs on a headless runner"]
    fn test_permissive_access_leaves_the_secret_readable() {
        let dir = TempDir::new().unwrap();
        let keychain = keychain_with_secret(&dir);

        set_permissive_access(Some(&keychain), SERVICE, ACCOUNT).expect(HEADLESS);

        let (password, _item) = find_generic_password(Some(&[keychain]), SERVICE, ACCOUNT).unwrap();
        assert_eq!(&*password, b"secret");
    }

    /// Which of the two writing calls blocks.
    ///
    /// This one stops before `SecKeychainItemSetAccess`: it builds the access
    /// object and rewrites its ACLs in memory, touching no stored item. If it
    /// passes where the two above hang, the block is attaching the access, not
    /// composing it.
    #[test]
    #[ignore = "probe: splits SecACLSetContents from SecKeychainItemSetAccess"]
    fn probe_building_permissive_access_does_not_block() {
        permissive_access(&CFString::new(SERVICE)).unwrap();
    }

    #[test]
    fn test_missing_item_is_an_error() {
        let dir = TempDir::new().unwrap();
        let keychain = keychain_with_secret(&dir);

        assert!(set_permissive_access(Some(&keychain), SERVICE, "nobody").is_err());
    }
}
