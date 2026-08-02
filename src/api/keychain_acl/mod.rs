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
//! writes a new item rather than amending one.
//!
//! # Why the read path has to check too
//!
//! Writing covers an item this build wrote, but grans mostly reads: a user
//! whose stored access token is still valid goes a whole command without
//! writing anything. An entry left by a grans from before any of this existed
//! is still pinned to whatever build wrote it, is still challenged on every
//! read, and no amount of writing that never happens will fix it.
//!
//! [`open_up_existing`] is the check that closes that gap. Locating an item and
//! reading its ACLs decrypts nothing and needs no authorization, so it costs
//! the user nothing on the common path where the item is already open, and it
//! rewrites only what would otherwise have gone on prompting forever.

mod sys;

use std::ffi::c_void;
use std::ptr;

use anyhow::{Result, bail};
use core_foundation::base::TCFType;
use core_foundation::string::CFString;
use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
use core_foundation_sys::base::{CFRelease, OSStatus};
use core_foundation_sys::string::CFStringRef;
use security_framework::os::macos::access::SecAccess;
use security_framework::os::macos::keychain::SecKeychain;
use security_framework::os::macos::keychain_item::SecKeychainItem;
use security_framework_sys::base::{
    SecAccessRef, SecKeychainAttribute, SecKeychainAttributeList, SecKeychainItemRef,
};

use sys::{
    ACCOUNT_ATTR, GENERIC_PASSWORD_CLASS, ITEM_NOT_FOUND, SERVICE_ATTR, SecACLCopyContents,
    SecACLRef, SecACLSetContents, SecAccessCopyACLList, SecAccessCreate,
    SecKeychainFindGenericPassword, SecKeychainItemCopyAccess, SecKeychainItemCreateFromContent,
    SecKeychainItemDelete, SecKeychainPromptSelector,
};

/// Store `secret` under `service`/`account` so any application can read it back
/// without a password prompt.
///
/// Replaces whatever is already stored there. See the module docs for why it
/// replaces rather than amends.
pub fn store_with_open_access(service: &str, account: &str, secret: &[u8]) -> Result<()> {
    store(None, service, account, secret)
}

/// Give a stored item the open ACL if it does not already have one, and report
/// whether that was necessary.
///
/// `secret` is what the item currently holds. Opening it up means replacing it
/// (see the module docs), so the value has to be written back.
pub fn open_up_existing(service: &str, account: &str, secret: &[u8]) -> Result<bool> {
    open_up(None, service, account, secret)
}

/// The same, against a specific keychain.
fn open_up(
    keychain: Option<&SecKeychain>,
    service: &str,
    account: &str,
    secret: &[u8],
) -> Result<bool> {
    // Scoped, so the item reference is gone before `store` deletes what it
    // points at.
    let pinned = match find_item(keychain, service, account)? {
        Some(item) => names_applications(&item)?,
        None => return Ok(false),
    };

    if !pinned {
        return Ok(false);
    }

    store(keychain, service, account, secret)?;
    Ok(true)
}

/// The same, against a specific keychain. Tests pass a throwaway one rather
/// than touching the login keychain.
fn store(
    keychain: Option<&SecKeychain>,
    service: &str,
    account: &str,
    secret: &[u8],
) -> Result<()> {
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

    Ok(Some(unsafe {
        SecKeychainItem::wrap_under_create_rule(item)
    }))
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

/// Whether the item still trusts a fixed list of applications, which is what
/// makes macOS challenge every build of grans but the one that wrote it.
fn names_applications(item: &SecKeychainItem) -> Result<bool> {
    let mut access: SecAccessRef = ptr::null_mut();
    check(
        unsafe { SecKeychainItemCopyAccess(item.as_concrete_TypeRef(), &mut access) },
        "read the access settings of the keychain entry",
    )?;
    let access = unsafe { SecAccess::wrap_under_create_rule(access) };

    // Bound rather than iterated inline: the entries are borrowed from the
    // array, so it has to outlive the loop.
    let acls = acl_list(&access)?;
    for acl in acls.entries() {
        if acl_names_applications(acl)? {
            return Ok(true);
        }
    }

    Ok(false)
}

/// Whether one ACL names applications. Any list at all is a fixed set; only a
/// NULL one means "every application".
fn acl_names_applications(acl: SecACLRef) -> Result<bool> {
    let mut applications: CFArrayRef = ptr::null();
    let mut description: CFStringRef = ptr::null();
    let mut prompt: SecKeychainPromptSelector = 0;

    check(
        unsafe { SecACLCopyContents(acl, &mut applications, &mut description, &mut prompt) },
        "read the contents of an ACL",
    )?;

    // Both out-params come back under the create rule, so both are ours to
    // release even though only one is being read.
    let applications = OwnedArray(applications);
    if !description.is_null() {
        unsafe { CFRelease(description.cast()) };
    }

    Ok(!applications.is_null())
}

/// Drop every ACL's trusted-application list.
fn clear_trusted_applications(access: &SecAccess, descriptor: &CFString) -> Result<()> {
    let acls = acl_list(access)?;
    for acl in acls.entries() {
        check(
            // NULL is what macOS reads as "any application, no prompt". An
            // empty array is the opposite: nobody is trusted, always ask.
            unsafe { SecACLSetContents(acl, ptr::null(), descriptor.as_concrete_TypeRef(), 0) },
            "clear the trusted-application list on an ACL",
        )?;
    }

    Ok(())
}

/// The ACLs of `access`, as an array that releases itself.
fn acl_list(access: &SecAccess) -> Result<OwnedArray> {
    let mut list: CFArrayRef = ptr::null();
    check(
        unsafe { SecAccessCopyACLList(access.as_concrete_TypeRef(), &mut list) },
        "read the ACLs of a keychain access object",
    )?;

    Ok(OwnedArray(list))
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

    /// Whether Security handed back no array at all, which for a
    /// trusted-application list is what "every application" looks like.
    fn is_null(&self) -> bool {
        self.0.is_null()
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

    /// Whether the stored item would challenge a build of grans other than the
    /// one that wrote it.
    fn is_pinned(keychain: &SecKeychain) -> bool {
        let item = find_item(Some(keychain), SERVICE, ACCOUNT)
            .unwrap()
            .unwrap();

        names_applications(&item).unwrap()
    }

    fn stored_secret(keychain: SecKeychain) -> Vec<u8> {
        let (password, _item) = find_generic_password(Some(&[keychain]), SERVICE, ACCOUNT).unwrap();

        password.to_vec()
    }

    #[test]
    fn test_default_write_is_pinned_to_its_creator() {
        // The premise of this whole module. Left to macOS, the item is pinned
        // to the binary that wrote it, whose ad-hoc signature changes with
        // every rebuild.
        let dir = TempDir::new().unwrap();
        let keychain = keychain_pinned_to_creator(&dir);

        assert!(is_pinned(&keychain));
    }

    #[test]
    fn test_stored_item_is_open_to_every_application() {
        let dir = TempDir::new().unwrap();
        let keychain = keychain(&dir);

        store(Some(&keychain), SERVICE, ACCOUNT, b"secret").unwrap();

        assert!(!is_pinned(&keychain));
    }

    #[test]
    fn test_stored_secret_reads_back() {
        let dir = TempDir::new().unwrap();
        let keychain = keychain(&dir);

        store(Some(&keychain), SERVICE, ACCOUNT, b"secret").unwrap();

        assert_eq!(stored_secret(keychain), b"secret");
    }

    #[test]
    fn test_store_replaces_an_earlier_value() {
        let dir = TempDir::new().unwrap();
        let keychain = keychain(&dir);

        store(Some(&keychain), SERVICE, ACCOUNT, b"first").unwrap();
        store(Some(&keychain), SERVICE, ACCOUNT, b"second").unwrap();

        assert_eq!(stored_secret(keychain), b"second");
    }

    #[test]
    fn test_store_opens_up_an_entry_left_pinned_by_an_older_grans() {
        // The upgrade path. Amending that item's ACL would need ChangeACL
        // against a signature that has since changed; replacing it does not.
        let dir = TempDir::new().unwrap();
        let keychain = keychain_pinned_to_creator(&dir);
        assert!(is_pinned(&keychain));

        store(Some(&keychain), SERVICE, ACCOUNT, b"rotated").unwrap();

        assert!(!is_pinned(&keychain));
        assert_eq!(stored_secret(keychain), b"rotated");
    }

    #[test]
    fn test_missing_item_is_not_an_error() {
        let dir = TempDir::new().unwrap();
        let keychain = keychain(&dir);

        assert!(
            find_item(Some(&keychain), SERVICE, "nobody")
                .unwrap()
                .is_none()
        );
    }

    // --- opening up what an older grans left behind ---

    #[test]
    fn test_open_up_rewrites_a_pinned_entry_and_keeps_its_secret() {
        // The bug this exists for: grans updates, the item it stored months
        // ago is still pinned to a build that no longer exists, and every read
        // is challenged. Nothing writes on a read-only command, so without
        // this the prompting never stops.
        let dir = TempDir::new().unwrap();
        let keychain = keychain_pinned_to_creator(&dir);

        let rewritten = open_up(Some(&keychain), SERVICE, ACCOUNT, b"secret").unwrap();

        assert!(rewritten);
        assert!(!is_pinned(&keychain));
        assert_eq!(stored_secret(keychain), b"secret");
    }

    #[test]
    fn test_open_up_leaves_an_already_open_entry_alone() {
        // Every command opens the credential store, so an item that is already
        // open must not be deleted and rewritten each time.
        let dir = TempDir::new().unwrap();
        let keychain = keychain(&dir);
        store(Some(&keychain), SERVICE, ACCOUNT, b"secret").unwrap();

        let rewritten = open_up(Some(&keychain), SERVICE, ACCOUNT, b"secret").unwrap();

        assert!(!rewritten);
        assert!(!is_pinned(&keychain));
    }

    #[test]
    fn test_open_up_reports_nothing_to_do_when_the_entry_is_gone() {
        // The item is looked up again here after having been read, so it can
        // have been removed in between.
        let dir = TempDir::new().unwrap();
        let keychain = keychain(&dir);

        assert!(!open_up(Some(&keychain), SERVICE, ACCOUNT, b"secret").unwrap());
    }
}
