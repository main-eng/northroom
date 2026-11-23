//! Authenticode checks for Windows updates. The setup must be fully signed, chain to a trusted
//! root with no revoked certificates below it, be signed by the same key as the running app (or
//! the key a check names), and carry the version the channel file offers, which the core has
//! already found newer. An unsigned running app accepts no updates. Keys are compared, not
//! names, so a certificate with the same name and a different key is a different signer. Every
//! signature is examined, so a dual-signed setup (old and renewed key, `signtool /as`) carries
//! an app across a certificate renewal. WinVerifyTrust shows no UI.

use northroom_core::update::ffi::NrSignature;
use northroom_core::update::sha512;
use windows::Win32::Foundation::{HWND, TRUST_E_NOSIGNATURE};
use windows::Win32::Security::Cryptography::CERT_CONTEXT;
use windows::Win32::Security::WinTrust::{
    WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO,
    WINTRUST_SIGNATURE_SETTINGS, WSS_GET_SECONDARY_SIG_COUNT, WSS_VERIFY_SPECIFIC, WTD_CHOICE_FILE,
    WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT, WTD_REVOKE_WHOLECHAIN, WTD_STATEACTION_CLOSE,
    WTD_STATEACTION_VERIFY, WTD_UI_NONE, WTHelperGetProvSignerFromChain,
    WTHelperProvDataFromStateData, WinVerifyTrust,
};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW,
};
use windows::core::{HRESULT, HSTRING, PCWSTR, w};

/// A failed check: WinVerifyTrust's status and its system message.
struct Untrusted {
    status: i32,
    reason: String,
}

/// A verified signature: the SHA-512 (base64) of the signer's public key, and how many other
/// signatures follow the first.
fn checked(path: &HSTRING, index: u32) -> Result<(String, u32), Untrusted> {
    let mut info = WINTRUST_FILE_INFO {
        cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: PCWSTR(path.as_ptr()),
        ..Default::default()
    };
    let mut settings = WINTRUST_SIGNATURE_SETTINGS {
        cbStruct: size_of::<WINTRUST_SIGNATURE_SETTINGS>() as u32,
        dwIndex: index,
        dwFlags: WSS_VERIFY_SPECIFIC | WSS_GET_SECONDARY_SIG_COUNT,
        ..Default::default()
    };
    let mut data = WINTRUST_DATA {
        cbStruct: size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_WHOLECHAIN,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 { pFile: &mut info },
        dwStateAction: WTD_STATEACTION_VERIFY,
        dwProvFlags: WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT,
        pSignatureSettings: &mut settings,
        ..Default::default()
    };
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    // Never show UI.
    let nobody = HWND(-1isize as *mut _);
    // SAFETY: `data` and its pointees outlive both calls; the state opened by the first call is
    // read before the second closes it.
    unsafe {
        let status = WinVerifyTrust(nobody, &mut action, (&raw mut data).cast());
        let key = if status == 0 {
            let provider = WTHelperProvDataFromStateData(data.hWVTStateData);
            let signer = WTHelperGetProvSignerFromChain(provider, 0, false, 0);
            if signer.is_null() || (*signer).csCertChain == 0 {
                Err(Untrusted {
                    status,
                    reason: "the signature names no certificate".to_owned(),
                })
            } else {
                Ok(key_digest((*(*signer).pasCertChain).pCert))
            }
        } else {
            Err(Untrusted {
                status,
                reason: HRESULT(status).message(),
            })
        };
        data.dwStateAction = WTD_STATEACTION_CLOSE;
        WinVerifyTrust(nobody, &mut action, (&raw mut data).cast());
        key.map(|key| (key, settings.cSecondarySigs))
    }
}

/// The SHA-512 (base64) of a certificate's public key, in the form checks use
/// (`[Convert]::ToBase64String(SHA512(cert.PublicKey.EncodedKeyValue.RawData))`).
///
/// # Safety
/// `certificate` is a live certificate context.
unsafe fn key_digest(certificate: *const CERT_CONTEXT) -> String {
    // SAFETY: per the caller; the key bytes belong to the certificate.
    let key = unsafe {
        let key = &(*(*certificate).pCertInfo).SubjectPublicKeyInfo.PublicKey;
        std::slice::from_raw_parts(key.pbData, key.cbData as usize)
    };
    let mut digest = sha512::Sha512::default();
    digest.update(key);
    sha512::base64(&digest.finish())
}

/// Keys of every passing signature on `file`; the first signature must pass.
fn signer_keys(file: &str) -> Result<Vec<String>, Untrusted> {
    let path = HSTRING::from(file);
    let (first, others) = checked(&path, 0)?;
    let mut keys = vec![first];
    keys.extend((1..=others).filter_map(|index| checked(&path, index).ok().map(|(key, _)| key)));
    Ok(keys)
}

/// A program's file version (`VS_FIXEDFILEINFO`) as `a.b.c.d`.
fn file_version(file: &str) -> Option<String> {
    let path = HSTRING::from(file);
    // SAFETY: the version block is read into a buffer of `size` bytes, and the fixed part is
    // read from inside it.
    unsafe {
        let size = GetFileVersionInfoSizeW(&path, None);
        if size == 0 {
            return None;
        }
        let mut block = vec![0u8; size as usize];
        GetFileVersionInfoW(&path, None, size, block.as_mut_ptr().cast()).ok()?;
        let mut fixed: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut length = 0u32;
        if !VerQueryValueW(block.as_ptr().cast(), w!("\\"), &mut fixed, &mut length).as_bool()
            || (length as usize) < size_of::<VS_FIXEDFILEINFO>()
        {
            return None;
        }
        let fixed = &*fixed.cast::<VS_FIXEDFILEINFO>();
        Some(format!(
            "{}.{}.{}.{}",
            fixed.dwFileVersionMS >> 16,
            fixed.dwFileVersionMS & 0xffff,
            fixed.dwFileVersionLS >> 16,
            fixed.dwFileVersionLS & 0xffff
        ))
    }
}

/// A release version as a four-part Windows file version (`0.3.1` → `0.3.1.0`).
fn as_file_version(version: &str) -> String {
    let release = version.split(['-', '+']).next().unwrap_or(version);
    let mut numbers: Vec<&str> = release.split('.').collect();
    numbers.resize(4, "0");
    numbers.join(".")
}

/// Checks the setup at `file` against `requirement` (a base64 SHA-512 of the signer's public
/// key, or the running app's signer when empty) and its version against the offered `version`.
/// Returns the result, WinVerifyTrust's status and its system message.
pub fn signature(file: &str, requirement: &str, version: &str) -> (NrSignature, i32, String) {
    let wanted = if requirement.is_empty() {
        let own = std::env::current_exe().map(|program| signer_keys(&program.to_string_lossy()));
        match own {
            Ok(Ok(keys)) => keys,
            _ => return (NrSignature::OwnUnreadable, 0, String::new()),
        }
    } else {
        vec![requirement.to_owned()]
    };
    let keys = match signer_keys(file) {
        Ok(keys) => keys,
        Err(Untrusted { status, .. }) if status == TRUST_E_NOSIGNATURE.0 => {
            return (NrSignature::Unreadable, status, String::new());
        }
        Err(Untrusted { status, reason }) => return (NrSignature::Failed, status, reason),
    };
    if !keys.iter().any(|key| wanted.contains(key)) {
        return (
            NrSignature::Failed,
            0,
            "it is signed with another key than this app's".to_owned(),
        );
    }
    let offered = as_file_version(version);
    match file_version(file) {
        Some(signed) if signed == offered => (NrSignature::Passed, 0, String::new()),
        signed => (
            NrSignature::Failed,
            0,
            format!(
                "it is version {}, not {offered}",
                signed.as_deref().unwrap_or("unknown")
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_release_version_is_written_as_a_file_version() {
        assert_eq!(as_file_version("0.3.1"), "0.3.1.0");
        assert_eq!(as_file_version("1.2"), "1.2.0.0");
        assert_eq!(as_file_version("0.4.0-beta.1"), "0.4.0.0");
    }

    #[test]
    fn an_unsigned_program_has_no_signer() {
        // The test program itself is built unsigned.
        let program = std::env::current_exe().unwrap();
        let Err(Untrusted { status, .. }) = signer_keys(&program.to_string_lossy()) else {
            panic!("an unsigned program has no signer");
        };
        assert_eq!(status, TRUST_E_NOSIGNATURE.0);
        assert_eq!(
            signature(&program.to_string_lossy(), "", "0.0.1").0,
            NrSignature::OwnUnreadable,
            "an unsigned app accepts no update"
        );
    }
}
