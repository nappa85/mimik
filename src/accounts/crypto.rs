use crate::AppError;
use argon2::{
    Argon2,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};
use std::sync::atomic::{AtomicBool, Ordering};
use zeroize::Zeroizing;

static BUSY: AtomicBool = AtomicBool::new(false);

// Public, valid canonical profile; never an enrolled account credential.
pub(super) const DUMMY_PHC: &str = "$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

pub(super) struct Permit {
    #[cfg(any(test, feature = "auth-test-support"))]
    pub(super) gate: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
    #[cfg(any(test, feature = "auth-test-support"))]
    pub(super) dropped: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
}
impl Permit {
    pub(super) fn acquire() -> Result<Self, AppError> {
        BUSY.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| AppError::PasswordBusy)?;
        Ok(Self {
            #[cfg(any(test, feature = "auth-test-support"))]
            gate: None,
            #[cfg(any(test, feature = "auth-test-support"))]
            dropped: None,
        })
    }
    #[cfg(any(test, feature = "auth-test-support"))]
    fn before_work(&self) {
        if let Some(gate) = &self.gate {
            gate();
        }
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
        #[cfg(any(test, feature = "auth-test-support"))]
        if let Some(dropped) = &self.dropped {
            dropped();
        }
    }
}

pub(super) fn password(bytes: &[u8]) -> Result<(), AppError> {
    password_for(bytes, false)
}

pub(super) fn password_for(bytes: &[u8], owner: bool) -> Result<(), AppError> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| AppError::Invalid("Password must be UTF-8."))?;
    if bytes.len() > 512
        || !((if owner { 15 } else { 12 })..=128).contains(&text.chars().count())
        || text.chars().any(char::is_control)
    {
        return Err(AppError::Invalid(if owner {
            "Owner password must contain 15–128 non-control Unicode scalars, at most 512 bytes."
        } else {
            "Player password must contain 12–128 non-control Unicode scalars, at most 512 bytes."
        }));
    }
    Ok(())
}

pub(super) fn admit(text: &str) -> Result<PasswordHash, AppError> {
    let invalid =
        || AppError::Invalid("Stored account verifier has unsupported integrity/profile.");
    if text.len() != 97 || !text.is_ascii() || !text.starts_with("$argon2id$v=19$m=19456,t=2,p=1$")
    {
        return Err(invalid());
    }
    let hash = PasswordHash::new(text).map_err(|_| invalid())?;
    // RustCrypto's canonical serializer rejects noncanonical encodings, including
    // nonzero unused base64 bits, reordered/extra parameters and padding.
    if hash.to_string() != text {
        return Err(invalid());
    }
    let parts: Vec<_> = text.split('$').collect();
    if parts.len() != 6 || parts[4].len() != 22 || parts[5].len() != 43 {
        return Err(invalid());
    }
    // Decode through the maintained PHC library rather than a custom codec.
    if hash.salt.as_ref().map(|s| s.len()) != Some(16)
        || hash.hash.as_ref().map(|h| h.as_bytes().len()) != Some(32)
    {
        return Err(invalid());
    }
    Ok(hash)
}

pub(super) async fn hash(
    bytes: Zeroizing<Vec<u8>>,
    permit: Permit,
) -> Result<(String, Permit), AppError> {
    tokio::task::spawn_blocking(move || {
        #[cfg(any(test, feature = "auth-test-support"))]
        permit.before_work();
        let hash = Argon2::default()
            .hash_password(&bytes)
            .map_err(|_| AppError::Invalid("Password hashing failed."))?
            .to_string();
        admit(&hash)?;
        Ok((hash, permit))
    })
    .await
    .map_err(|_| AppError::Worker)?
}

pub(super) async fn verify(
    bytes: Zeroizing<Vec<u8>>,
    text: String,
    permit: Permit,
) -> Result<(bool, Permit), AppError> {
    tokio::task::spawn_blocking(move || {
        let parsed = admit(&text)?;
        #[cfg(any(test, feature = "auth-test-support"))]
        permit.before_work();
        let matched = match Argon2::default().verify_password(&bytes, &parsed) {
            Ok(()) => true,
            Err(argon2::password_hash::Error::PasswordInvalid) => false,
            Err(_) => return Err(AppError::Invalid("Password verification failed.")),
        };
        Ok((matched, permit))
    })
    .await
    .map_err(|_| AppError::Worker)?
}
