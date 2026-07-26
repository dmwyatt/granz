//! Raw Security framework bindings the crates in use do not provide.
//!
//! `security-framework` declares the `SecAccess` type and nothing that builds
//! or inspects one, and `security-framework-sys` covers only
//! `SecAccessGetTypeID`, so the ACL calls have to be declared here. Everything
//! in this module is a verbatim transcription of `<Security/Security.h>`; the
//! policy that uses it lives in the parent.

use std::ffi::c_void;

use core_foundation_sys::array::CFArrayRef;
use core_foundation_sys::base::{CFTypeRef, OSStatus};
use core_foundation_sys::string::CFStringRef;
use security_framework_sys::base::{SecAccessRef, SecKeychainAttributeList, SecKeychainItemRef};

/// An entry in a `SecAccess`'s ACL list. Opaque; only handed back to Security.
pub type SecACLRef = *const c_void;

/// `SecKeychainPromptSelector`, a bitfield of the conditions that force a
/// prompt even for a trusted caller. Zero forces none of them.
pub type SecKeychainPromptSelector = u16;

/// `errSecItemNotFound`. A miss, rather than something going wrong.
pub const ITEM_NOT_FOUND: OSStatus = -25300;

// Four-character codes, spelled out because the `-sys` crate binds none of
// them: 'genp' (a generic password), 'svce' (its service), 'acct' (its
// account).
pub const GENERIC_PASSWORD_CLASS: u32 = 0x6765_6E70;
pub const SERVICE_ATTR: u32 = 0x7376_6365;
pub const ACCOUNT_ATTR: u32 = 0x6163_6374;

unsafe extern "C" {
    pub fn SecAccessCreate(
        descriptor: CFStringRef,
        trustedlist: CFArrayRef,
        accessRef: *mut SecAccessRef,
    ) -> OSStatus;

    pub fn SecAccessCopyACLList(accessRef: SecAccessRef, aclList: *mut CFArrayRef) -> OSStatus;

    pub fn SecACLSetContents(
        acl: SecACLRef,
        applicationList: CFArrayRef,
        description: CFStringRef,
        promptSelector: SecKeychainPromptSelector,
    ) -> OSStatus;

    pub fn SecACLCopyContents(
        acl: SecACLRef,
        applicationList: *mut CFArrayRef,
        description: *mut CFStringRef,
        promptSelector: *mut SecKeychainPromptSelector,
    ) -> OSStatus;

    pub fn SecKeychainItemCopyAccess(
        itemRef: SecKeychainItemRef,
        accessRef: *mut SecAccessRef,
    ) -> OSStatus;

    pub fn SecKeychainFindGenericPassword(
        keychainOrArray: CFTypeRef,
        serviceNameLength: u32,
        serviceName: *const c_void,
        accountNameLength: u32,
        accountName: *const c_void,
        passwordLength: *mut u32,
        passwordData: *mut *mut c_void,
        itemRef: *mut SecKeychainItemRef,
    ) -> OSStatus;

    pub fn SecKeychainItemCreateFromContent(
        itemClass: u32,
        attrList: *const SecKeychainAttributeList,
        length: u32,
        data: *const c_void,
        keychainRef: CFTypeRef,
        initialAccess: SecAccessRef,
        itemRef: *mut SecKeychainItemRef,
    ) -> OSStatus;

    pub fn SecKeychainItemDelete(itemRef: SecKeychainItemRef) -> OSStatus;
}
