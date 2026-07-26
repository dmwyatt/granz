//! Storing grans's session where macOS will hand it back without a prompt.
//!
//! macOS decides who may read a keychain item from the *code signature* of the
//! application that created it, not from its path. A cargo-built binary has no
//! stable one: on Apple Silicon the linker applies an ad-hoc signature, which
//! carries no signing identity, so the designated requirement degrades to a
//! hash of those exact bytes. Every rebuild is a different application as far
//! as the keychain is concerned, the ACL written during `grans auth login`
//! matches one build and nothing after it, and "Always Allow" only ever pins
//! whichever binary happens to be running at the time.
//!
//! So the item is created carrying the permissive ACL that
//! `security add-generic-password -A` writes: a trusted-application list of
//! NULL, which macOS reads as "any application, no prompt". The refresh token
//! stays encrypted at rest and out of readable backups, which is what the
//! keychain buys over the `0600` fallback file, but any process running as the
//! user can read it without being challenged.
//!
//! # Why it creates rather than amends
//!
//! Attaching that ACL to an item that already exists means `ChangeACL`, and
//! `ChangeACL` is the one authorization grans can least rely on: it is granted
//! against the same unstable signature the ACL is being rewritten to stop
//! depending on. Where a prompt can be drawn that costs the user a password;
//! where one cannot, `SecKeychainItemSetAccess` does not fail, it never
//! returns. A CI run measured that as a test binary held open for 22 minutes
//! against a 38 second baseline.
//!
//! Creating the item with its access already attached needs no such
//! authorization, so [`store_with_open_access`] deletes whatever is there and
//! writes a new item rather than amending one. That also upgrades an entry left
//! by an older grans, without the prompt an amend would have cost.

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
use security_framework_sys::base::{
    SecAccessRef, SecKeychainAttribute, SecKeychainAttributeList, SecKeychainItemRef,
};

/// An entry in a `SecAccess`'s ACL list. Opaque; only handed back to Security.
type SecACLRef = *const c_void;

/// `SecKeychainPromptSelector`, a bitfield of the conditions that force a
/// prompt even for a trusted caller. Zero forces none of them.
type SecKeychainPromptSelector = u16;

/// `errSecItemNotFound`. A miss, rather than something going wrong.
const ITEM_NOT_FOUND: OSStatus = -25300;

// Four-character codes, spelled out because the `-sys` crate binds none of
// them: 'genp' (a generic password), 'svce' (its service), 'acct' (its
// account).
const GENERIC_PASSWORD_CLASS: u32 = 0x6765_6E70;
const SERVICE_ATTR: u32 = 0x7376_6365;
const ACCOUNT_ATTR: u32 = 0x6163_6374;

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

    fn SecKeychainItemCreateFromContent(
        itemClass: u32,
        attrList: *const SecKeychainAttributeList,
        length: u32,
        data: *const c_void,
        keychainRef: CFTypeRef,
        initialAccess: SecAccessRef,
        itemRef: *mut SecKeychainItemRef,
    ) -> OSStatus;

    fn SecKeychainItemDelete(itemRef: SecKeychainItemRef) -> OSStatus;
}

/// Store `secret` under `service`/`account` so any application can read it back
/// without a password prompt.
///
/// Replaces whatever is already stored there. See the module docs for why it
/// replaces rather than amends.
pub fn store_with_open_access(service: &str, account: &str, secret: &[u8]) -> Result<()> {
    store(None, service, account, secret)
}

/// The same, against a specific keychain. Tests pass a throwaway one rather
/// than touching the login keychain.
fn store(keychain: Option<&SecKeychain>, service: &str, account: &str, secret: &[u8]) -> Result<()> {
    if let Some(existing) = find_item(keychain, service, account)? {
        check(
            unsafe { SecKeychainItemDelete(existing.as_concrete_TypeRef()) },
            "remove the keychain entry being replaced",
        )?;
    }

    let access = permissive_access(&CFString::new(service))?;
    create_item(keychain, service, account, secret, &access)
}

/// Write a new generic password carrying `access` from the moment it exists.
fn create_item(
    keychain: Option<&SecKeychain>,
    service: &str,
    account: &str,
    secret: &[u8],
    access: &SecAccess,
) -> Result<()> {
    let mut attributes = [
        SecKeychainAttribute {
            tag: SERVICE_ATTR,
            length: service.len() as u32,
            data: service.as_ptr() as *mut c_void,
        },
        SecKeychainAttribute {
            tag: ACCOUNT_ATTR,
            length: account.len() as u32,
            data: account.as_ptr() as *mut c_void,
        },
    ];
    let list = SecKeychainAttributeList {
        count: attributes.len() as u32,
        attr: attributes.as_mut_ptr(),
    };

    check(
        unsafe {
            SecKeychainItemCreateFromContent(
                GENERIC_PASSWORD_CLASS,
                &list,
                secret.len() as u32,
                secret.as_ptr().cast(),
                // A null keychain means the user's default one.
                keychain.map_or(ptr::null(), |keychain| keychain.as_CFTypeRef()),
                access.as_concrete_TypeRef(),
                ptr::null_mut(),
            )
        },
        "write the keychain entry",
    )
}

/// Locate the stored item without decrypting it, or `None` if there is none.
///
/// `security-framework` only exposes a lookup that returns the secret too, and
/// reading the secret is its own keychain authorization, spent here on a value
/// this has no use for.
fn find_item(
    keychain: Option<&SecKeychain>,
    service: &str,
    account: &str,
) -> Result<Option<SecKeychainItem>> {
    let mut item: SecKeychainItemRef = ptr::null_mut();

    let status = unsafe {
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
    };

    if status == ITEM_NOT_FOUND {
        return Ok(None);
    }
    check(status, "look for grans's entry in the keychain")?;

    Ok(Some(unsafe { SecKeychainItem::wrap_under_create_rule(item) }))
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

    /// A throwaway keychain, so the tests never touch the login keychain.
    fn keychain(dir: &TempDir) -> SecKeychain {
        CreateOptions::new()
            .password("test-password")
            .create(dir.path().join("grans-test.keychain"))
            .unwrap()
    }

    /// A keychain holding an item written the way macOS does by default, which
    /// pins it to whichever binary created it.
    fn keychain_pinned_to_creator(dir: &TempDir) -> SecKeychain {
        let keychain = keychain(dir);
        keychain
            .set_generic_password(SERVICE, ACCOUNT, b"secret")
            .unwrap();
        keychain
    }

    /// For each ACL on the stored item, whether it trusts a fixed list of
    /// applications rather than all of them.
    fn acls_naming_applications(keychain: &SecKeychain) -> Vec<bool> {
        let item = find_item(Some(keychain), SERVICE, ACCOUNT).unwrap().unwrap();

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
    fn test_default_write_is_pinned_to_its_creator() {
        // The premise of this whole module. Left to macOS, the item is pinned
        // to the binary that wrote it, whose ad-hoc signature changes with
        // every rebuild.
        let dir = TempDir::new().unwrap();
        let keychain = keychain_pinned_to_creator(&dir);

        assert!(acls_naming_applications(&keychain).contains(&true));
    }

    #[test]
    fn test_stored_item_is_open_to_every_application() {
        let dir = TempDir::new().unwrap();
        let keychain = keychain(&dir);

        store(Some(&keychain), SERVICE, ACCOUNT, b"secret").unwrap();

        assert!(!acls_naming_applications(&keychain).contains(&true));
    }

    #[test]
    fn test_stored_secret_reads_back() {
        let dir = TempDir::new().unwrap();
        let keychain = keychain(&dir);

        store(Some(&keychain), SERVICE, ACCOUNT, b"secret").unwrap();

        let (password, _item) = find_generic_password(Some(&[keychain]), SERVICE, ACCOUNT).unwrap();
        assert_eq!(&*password, b"secret");
    }

    #[test]
    fn test_store_replaces_an_earlier_value() {
        let dir = TempDir::new().unwrap();
        let keychain = keychain(&dir);

        store(Some(&keychain), SERVICE, ACCOUNT, b"first").unwrap();
        store(Some(&keychain), SERVICE, ACCOUNT, b"second").unwrap();

        let (password, _item) = find_generic_password(Some(&[keychain]), SERVICE, ACCOUNT).unwrap();
        assert_eq!(&*password, b"second");
    }

    #[test]
    fn test_store_opens_up_an_entry_left_pinned_by_an_older_grans() {
        // The upgrade path. Amending that item's ACL would need ChangeACL
        // against a signature that has since changed; replacing it does not.
        let dir = TempDir::new().unwrap();
        let keychain = keychain_pinned_to_creator(&dir);
        assert!(acls_naming_applications(&keychain).contains(&true));

        store(Some(&keychain), SERVICE, ACCOUNT, b"rotated").unwrap();

        assert!(!acls_naming_applications(&keychain).contains(&true));
        let (password, _item) = find_generic_password(Some(&[keychain]), SERVICE, ACCOUNT).unwrap();
        assert_eq!(&*password, b"rotated");
    }

    #[test]
    fn test_missing_item_is_not_an_error() {
        let dir = TempDir::new().unwrap();
        let keychain = keychain(&dir);

        assert!(find_item(Some(&keychain), SERVICE, "nobody")
            .unwrap()
            .is_none());
    }
}
