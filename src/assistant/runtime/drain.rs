//! The child's output (assisted sheet plan §1.4a, "Output"): stdout and stderr are each
//! drained by a reader thread with a 4 KiB line bound (longer lines are cut) and never copied
//! into Mimik's log. Only fixed patterns become events: listening, the build number, the
//! offload line, and model-load and device errors.
use std::io::Read;
use std::sync::LazyLock;

/// The longest line kept for matching; the rest of a longer line is discarded unread into
/// memory.
pub(crate) const LINE_BOUND: usize = 4096;

/// What a line of the child's output can signal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Output {
    /// `llama_server: listening on …` (a hint for when to connect, never the proof).
    Listening,
    /// `build: <n>` or `(build <n>, …)`.
    Build(u32),
    /// `offloaded <n>/<m> layers to GPU` (2c-2b checks n = m).
    Offloaded { layers: u32, of: u32 },
    /// The model could not be loaded.
    ModelLoadError,
    /// A backend or device could not be initialised.
    DeviceError,
    /// bubblewrap's own error (`bwrap: …`): the sandbox could not be set up.
    SandboxError,
    /// The server could not bind its socket or port.
    BindError,
}

static PATTERNS: LazyLock<[regex::Regex; 7]> = LazyLock::new(|| {
    [
        regex::Regex::new(r"^bwrap: ").expect("pattern"),
        regex::Regex::new(r"(?i)(couldn't bind|could not bind|failed to bind|address already in use)")
            .expect("pattern"),
        regex::Regex::new(r"llama_server: listening on ").expect("pattern"),
        regex::Regex::new(r"\bbuild[: ]+(\d{1,9})\b").expect("pattern"),
        regex::Regex::new(r"offloaded (\d{1,6})/(\d{1,6}) layers to GPU").expect("pattern"),
        regex::Regex::new(r"(?i)(failed to load model|error loading model|unable to load model)")
            .expect("pattern"),
        regex::Regex::new(
            r"(?i)(failed to initialize (the )?(vulkan|backend|device)|ggml_vulkan: (error|no devices))",
        )
        .expect("pattern"),
    ]
});

/// The event one line signals, if any.
pub(crate) fn classify(line: &str) -> Option<Output> {
    let [sandbox, bind, listening, build, offloaded, model, device] = &*PATTERNS;
    if sandbox.is_match(line) {
        return Some(Output::SandboxError);
    }
    if bind.is_match(line) {
        return Some(Output::BindError);
    }
    if listening.is_match(line) {
        return Some(Output::Listening);
    }
    if let Some(captures) = offloaded.captures(line) {
        return Some(Output::Offloaded {
            layers: captures[1].parse().ok()?,
            of: captures[2].parse().ok()?,
        });
    }
    if model.is_match(line) {
        return Some(Output::ModelLoadError);
    }
    if device.is_match(line) {
        return Some(Output::DeviceError);
    }
    build
        .captures(line)
        .and_then(|captures| captures[1].parse().ok())
        .map(Output::Build)
}

/// Live tests read the child's lines here (the marker check); nothing else ever does.
#[cfg(test)]
pub(crate) static TAP: std::sync::Mutex<Option<Vec<String>>> = std::sync::Mutex::new(None);

/// Reads `reader` to its end, cutting lines at [`LINE_BOUND`], and calls `event` for each
/// line that signals something. Memory stays bounded whatever the child writes.
pub(crate) fn drain(mut reader: impl Read, mut event: impl FnMut(Output)) {
    let mut line: Vec<u8> = Vec::with_capacity(LINE_BOUND);
    let mut skipping = false;
    let mut chunk = [0_u8; 8192];
    let finish = |line: &mut Vec<u8>, event: &mut dyn FnMut(Output)| {
        #[cfg(test)]
        if let Some(tap) = TAP
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_mut()
        {
            tap.push(String::from_utf8_lossy(line).into_owned());
        }
        if let Some(found) = classify(&String::from_utf8_lossy(line)) {
            event(found);
        }
        line.clear();
    };
    loop {
        let read = match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        for &byte in &chunk[..read] {
            if byte == b'\n' {
                if !skipping {
                    finish(&mut line, &mut event);
                }
                skipping = false;
            } else if !skipping {
                line.push(byte);
                if line.len() >= LINE_BOUND {
                    finish(&mut line, &mut event);
                    skipping = true;
                }
            }
        }
    }
    if !line.is_empty() {
        finish(&mut line, &mut event);
    }
}

/// Drains `reader` on its own thread, sending events to `events`.
pub(crate) fn spawn(
    name: &str,
    reader: impl Read + Send + 'static,
    events: tokio::sync::mpsc::UnboundedSender<Output>,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name(format!("assistant-{name}"))
        .spawn(move || {
            drain(reader, |event| {
                let _ = events.send(event);
            });
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_patterns_from_the_pinned_build() {
        assert_eq!(
            classify("0.00.029.302 I srv  llama_server: listening on unix:///run/llama/llama.sock"),
            Some(Output::Listening)
        );
        assert_eq!(
            classify("version: 0.6.0-dev (build 11609, commit fcac19079)"),
            Some(Output::Build(11609))
        );
        assert_eq!(
            classify("load_tensors: offloaded 41/41 layers to GPU"),
            Some(Output::Offloaded { layers: 41, of: 41 })
        );
        assert_eq!(
            classify("srv    load_model: failed to load model, '/models/model.gguf'"),
            Some(Output::ModelLoadError)
        );
        assert_eq!(
            classify("0.03.067.149 I slot print_timing: id  0 | task 0 | prompt eval time"),
            None
        );
        assert_eq!(
            classify("bwrap: Can't find source path /nonexistent: No such file or directory"),
            Some(Output::SandboxError)
        );
        assert_eq!(
            classify("srv    init: couldn't bind HTTP server socket, hostname: 127.0.0.1, port: 9"),
            Some(Output::BindError)
        );
        // The CPU build's note on the CPU path is not a device error.
        assert_eq!(
            classify("warning: no usable GPU found, --gpu-layers option will be ignored"),
            None
        );
    }

    /// A reader of `total` bytes of `byte`, produced as read (never held in memory).
    struct Endless {
        left: usize,
        byte: u8,
    }

    impl Read for Endless {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = buf.len().min(self.left);
            buf[..n].fill(self.byte);
            self.left -= n;
            Ok(n)
        }
    }

    #[test]
    fn a_line_of_a_mebibyte_and_endless_lines_stay_bounded() {
        // One line of 1 MiB with a pattern past the bound: cut, so it is not seen.
        let mut long = vec![b'x'; 1 << 20];
        long.extend_from_slice(b" llama_server: listening on x\n");
        let mut seen = Vec::new();
        drain(long.as_slice(), |event| seen.push(event));
        assert_eq!(seen, []);
        // 8 MiB without a newline: drained without growing.
        drain(
            Endless {
                left: 8 << 20,
                byte: b'y',
            },
            |event| seen.push(event),
        );
        // Endless short lines.
        drain(
            Endless {
                left: 4 << 20,
                byte: b'\n',
            },
            |event| seen.push(event),
        );
        assert_eq!(seen, []);
        // A pattern after a cut line is still found.
        let mut text = vec![b'z'; 10_000];
        text.extend_from_slice(b"\nllama_server: listening on unix:///x\n");
        drain(text.as_slice(), |event| seen.push(event));
        assert_eq!(seen, vec![Output::Listening]);
    }
}
