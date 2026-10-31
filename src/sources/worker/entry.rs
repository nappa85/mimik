use super::{
    Error, Input, inspect, operations,
    records::{Operation, RecordWriter},
};
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RenderSettings {
    pub dpi: u16,
    pub colourspace: String,
    pub anti_aliasing: u8,
}
impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            dpi: 150,
            colourspace: "rgb".into(),
            anti_aliasing: 8,
        }
    }
}

/// Internal spec, bounded at 64 KiB before parsing. Paths are never included in errors.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub operation: Operation,
    pub pages: Vec<u32>,
    pub settings: RenderSettings,
    pub input: PathBuf,
}

/// Run on the initial, single-threaded process before any runtime or PDF context exists.
///
/// # Errors
/// Returns fixed worker reasons only.
pub fn run_entry(spec_path: &Path) -> Result<(), Error> {
    let mut spec_bytes = Vec::new();
    std::fs::File::open(spec_path)
        .map_err(|_| Error::Unreadable)?
        .take(65537)
        .read_to_end(&mut spec_bytes)
        .map_err(|_| Error::Unreadable)?;
    if spec_bytes.len() > 65536 {
        return Err(Error::TooLarge);
    }
    let spec: Spec = serde_json::from_slice(&spec_bytes).map_err(|_| Error::Unreadable)?;
    if spec.settings != RenderSettings::default()
        || spec.operation == Operation::Inspect && !spec.pages.is_empty()
        || spec.operation == Operation::Render && (spec.pages.is_empty() || spec.pages.len() > 8)
    {
        return Err(Error::Unsupported);
    }
    let output = protect_stdout()?;
    let mut writer = RecordWriter::new(output, spec.operation, spec.pages.len());
    let result = (|| {
        let mut bytes = Vec::new();
        std::fs::File::open(&spec.input)
            .map_err(|_| Error::Unreadable)?
            .take(128 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Unreadable)?;
        if bytes.len() > 128 * 1024 * 1024 {
            return Err(Error::TooLarge);
        }
        // Slice 2b installs its second filter here: /in is closed, MuPDF not yet called.
        match spec.operation {
            Operation::Inspect => writer.inspection(&inspect(Input::Bytes(&bytes))?),
            Operation::Render => {
                operations::render_each(Input::Bytes(&bytes), &spec.pages, |p| writer.rendered(&p))
            }
        }
    })();
    complete(writer, result)
}

pub(super) fn complete<W: std::io::Write>(
    writer: RecordWriter<W>,
    result: Result<(), Error>,
) -> Result<(), Error> {
    match result {
        Ok(()) => writer.finish(),
        Err(reason) => {
            // The terminal frame is best effort; preserve the original process status
            // even if its budget is exhausted or the parent has closed the pipe.
            let _ = writer.error(reason);
            Err(reason)
        }
    }
}

fn protect_stdout() -> Result<std::fs::File, Error> {
    use std::os::fd::FromRawFd;
    // SAFETY: invoked by the worker entry before any threads or MuPDF calls. fcntl creates
    // an exclusively owned descriptor, wrapped exactly once. dup2 changes only fd 1;
    // failure closes the owned duplicate through File's destructor.
    unsafe {
        let fd = libc::fcntl(libc::STDOUT_FILENO, libc::F_DUPFD_CLOEXEC, 3);
        if fd < 0 {
            return Err(Error::Unreadable);
        }
        let file = std::fs::File::from_raw_fd(fd);
        if libc::dup2(libc::STDERR_FILENO, libc::STDOUT_FILENO) < 0 {
            return Err(Error::Unreadable);
        }
        Ok(file)
    }
}
