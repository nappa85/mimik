//! The per-launch token (assisted sheet plan §1.4a, "Token"): 32 bytes from the OS random
//! source, base64url, new for every launch, held in memory and zeroed when dropped. It never
//! appears on a command line or in a log. Channels: level 1 binds it into the sandbox from a
//! pipe (`--ro-bind-data`) for `--api-key-file`; the TCP levels write it to a 0600 file in the
//! generation's 0700 directory, deleted once `/health` answers.
use base64::Engine;
use std::io::Write;
use std::os::fd::{AsFd, BorrowedFd};
use zeroize::Zeroizing;

/// The token file's name in the generation's directory (TCP levels).
pub(crate) const FILE: &str = "token";

pub(crate) struct Token(Zeroizing<String>);

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token(…)")
    }
}

impl Token {
    /// A fresh token.
    pub(crate) fn new() -> Self {
        let mut bytes = Zeroizing::new([0_u8; 32]);
        getrandom::fill(bytes.as_mut_slice()).expect("the OS random source is available");
        Self(Zeroizing::new(
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes.as_slice()),
        ))
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }

    /// The file content `--api-key-file` reads: the key and a newline.
    pub(crate) fn line(&self) -> Zeroizing<Vec<u8>> {
        let mut line = Zeroizing::new(Vec::with_capacity(self.0.len() + 1));
        line.extend_from_slice(self.0.as_bytes());
        line.push(b'\n');
        line
    }

    /// Writes the token file into the generation's directory: created exclusively, mode
    /// 0600, never following a link.
    pub(crate) fn write_file(&self, directory: BorrowedFd<'_>) -> std::io::Result<()> {
        use rustix::fs::{Mode, OFlags};
        let fd = rustix::fs::openat(
            directory,
            FILE,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )?;
        let mut file = std::fs::File::from(fd);
        file.write_all(&self.line())?;
        file.sync_all()
    }
}

/// Deletes the token file (once `/health` answered, or when the generation ends).
pub(crate) fn remove_file(directory: &impl AsFd) {
    let _ = rustix::fs::unlinkat(directory, FILE, rustix::fs::AtFlags::empty());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn tokens_are_32_random_bytes_in_base64url_and_never_debug_printed() {
        let one = Token::new();
        let two = Token::new();
        assert_eq!(one.expose().len(), 43);
        assert!(
            one.expose()
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
        assert_ne!(one.expose(), two.expose());
        assert_eq!(format!("{one:?}"), "Token(…)");
    }

    #[test]
    fn the_file_holds_the_key_line() {
        let token = Token::new();
        let dir = tempfile::tempdir().unwrap();
        let fd = rustix::fs::open(
            dir.path(),
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::DIRECTORY,
            rustix::fs::Mode::empty(),
        )
        .unwrap();
        token.write_file(fd.as_fd()).unwrap();
        let path = dir.path().join(FILE);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            format!("{}\n", token.expose())
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        // Never over an existing file or through a link.
        assert!(token.write_file(fd.as_fd()).is_err());
        remove_file(&fd);
        assert!(!path.exists());
        std::os::unix::fs::symlink("/dev/null", &path).unwrap();
        assert!(token.write_file(fd.as_fd()).is_err());
    }
}
