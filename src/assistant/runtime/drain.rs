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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    /// The server could not bind its socket or port.
    BindError,
}

static PATTERNS: LazyLock<[regex::Regex; 6]> = LazyLock::new(|| {
    [
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
    let [bind, listening, build, offloaded, model, device] = &*PATTERNS;
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

/// Bounded, child-local raw output capture for live confidentiality checks. A full capture
/// fails the check rather than silently discarding evidence.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct Capture {
    pub bytes: [Vec<u8>; 2],
    pub overflow: bool,
}

#[cfg(test)]
pub(crate) fn captured<R: Read>(
    name: &str,
    reader: R,
    capture: &std::sync::Arc<std::sync::Mutex<Capture>>,
) -> impl Read + use<R> {
    struct Captured<R> {
        reader: R,
        capture: std::sync::Arc<std::sync::Mutex<Capture>>,
        stream: usize,
    }
    impl<R: Read> Read for Captured<R> {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let read = self.reader.read(buffer)?;
            let mut capture = self.capture.lock().unwrap();
            let kept = read.min((1_usize << 20).saturating_sub(capture.bytes[self.stream].len()));
            capture.bytes[self.stream].extend_from_slice(&buffer[..kept]);
            capture.overflow |= kept != read;
            Ok(read)
        }
    }
    Captured {
        reader,
        capture: std::sync::Arc::clone(capture),
        stream: usize::from(name == "stderr"),
    }
}

/// Reads `reader` to its end, cutting lines at [`LINE_BOUND`], and calls `event` for each
/// line that signals something. Memory stays bounded whatever the child writes.
pub(crate) fn drain(mut reader: impl Read, mut event: impl FnMut(Output)) {
    let mut line: Vec<u8> = Vec::with_capacity(LINE_BOUND);
    let mut skipping = false;
    let mut chunk = [0_u8; 8192];
    let finish = |line: &mut Vec<u8>, event: &mut dyn FnMut(Output)| {
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

/// What the child's output signalled so far: a fixed-size summary, so memory stays bounded
/// however many matching lines the child prints.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Summary {
    pub listening: bool,
    pub failed: Failed,
    pub build: Option<u32>,
    pub offloaded: Option<(u32, u32)>,
}

/// The failures the child's output reported.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Failed {
    pub model_load: bool,
    pub device: bool,
    pub bind: bool,
}

impl Summary {
    pub(crate) fn record(&mut self, event: Output) {
        match event {
            Output::Listening => self.listening = true,
            Output::ModelLoadError => self.failed.model_load = true,
            Output::DeviceError => self.failed.device = true,
            Output::BindError => self.failed.bind = true,
            Output::Build(build) => self.build = Some(build),
            Output::Offloaded { layers, of } => self.offloaded = Some((layers, of)),
        }
    }
}

/// Drains `reader` on its own thread into `summary`, then calls `done`.
pub(crate) fn spawn(
    name: &str,
    reader: impl Read + Send + 'static,
    summary: &std::sync::Arc<std::sync::Mutex<Summary>>,
    done: impl FnOnce() + Send + 'static,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    let summary = std::sync::Arc::clone(summary);
    std::thread::Builder::new()
        .name(format!("assistant-{name}"))
        .spawn(move || {
            drain(reader, |event| {
                summary
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .record(event);
            });
            done();
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_children_and_streams_cannot_erase_or_split_captured_evidence() {
        use std::sync::{Arc, Mutex};
        let first = Arc::new(Mutex::new(Capture::default()));
        let mut stdout = captured("stdout", &b"secret-marker"[..], &first);
        let mut buffer = [0; 7];
        assert_eq!(stdout.read(&mut buffer).unwrap(), 7);
        let second = Arc::new(Mutex::new(Capture::default()));
        drain(captured("stderr", &b"unrelated\n"[..], &second), |_| {});
        drain(captured("stderr", &b"noise\n"[..], &first), |_| {});
        drain(stdout, |_| {});
        let first = first.lock().unwrap();
        assert_eq!(first.bytes[0], b"secret-marker");
        assert_eq!(first.bytes[1], b"noise\n");
        assert!(!first.overflow);
        assert_eq!(second.lock().unwrap().bytes[1], b"unrelated\n");
    }

    #[test]
    fn a_capture_that_cannot_keep_all_evidence_reports_overflow() {
        use std::sync::{Arc, Mutex};
        let capture = Arc::new(Mutex::new(Capture::default()));
        drain(
            captured(
                "stdout",
                Endless {
                    left: (1 << 20) + 1,
                    byte: b'x',
                },
                &capture,
            ),
            |_| {},
        );
        let capture = capture.lock().unwrap();
        assert_eq!(capture.bytes[0].len(), 1 << 20);
        assert!(capture.overflow);
    }

    #[test]
    fn request_text_in_the_output_at_any_verbosity_reaches_nothing_but_the_fixed_summary() {
        // At higher verbosity the server may log request contents. The drain's only outputs
        // are the fixed events (numbers and flags), and the summary has no text field.
        let marker = "ZZ-PLAYER-MARKER-7731";
        let log = format!(
            "srv  log_server_r: request: POST /v1/chat/completions {{\"messages\":[{{\"content\":\"{marker}\"}}]}}\n\
             slot update_slots: prompt = {marker}\n\
             slot launch_slot_: id 0 | task 0 | processing task, text: {marker}\n"
        );
        let mut events = Vec::new();
        drain(log.as_bytes(), |event| events.push(event));
        assert!(events.is_empty(), "{events:?}");
        // A phrase shaped like a pattern yields the numeric event alone, never its text.
        let shaped = format!("prompt: {marker} offloaded 1/2 layers to GPU\n");
        let mut summary = Summary::default();
        drain(shaped.as_bytes(), |event| summary.record(event));
        assert_eq!(summary.offloaded, Some((1, 2)));
        assert!(!format!("{summary:?}").contains(marker));
        assert!(std::mem::size_of::<Summary>() <= 32);
    }

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
        // The child's output never says the sandbox failed: that is the shim's marker alone.
        assert_eq!(
            classify("bwrap: Can't find source path /nonexistent: No such file or directory"),
            None
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
    fn a_flood_of_matching_lines_keeps_a_fixed_summary() {
        // A million matching lines through the real collector: a fixed-size summary, and the
        // thread reports its end.
        let mut text = Vec::new();
        for n in 0..1_000_000_u32 {
            text.extend_from_slice(format!("build: {}\n", n % 7).as_bytes());
        }
        text.extend_from_slice(
            b"llama_server: listening on x\nload_tensors: offloaded 3/4 layers to GPU\n",
        );
        let summary = std::sync::Arc::new(std::sync::Mutex::new(Summary::default()));
        let (sender, ended) = std::sync::mpsc::channel();
        spawn("test", std::io::Cursor::new(text), &summary, move || {
            sender.send(()).unwrap();
        })
        .unwrap()
        .join()
        .unwrap();
        ended.recv().unwrap();
        let summary = *summary.lock().unwrap();
        assert_eq!(
            summary,
            Summary {
                listening: true,
                build: Some(999_999 % 7),
                offloaded: Some((3, 4)),
                ..Summary::default()
            }
        );
        assert!(std::mem::size_of::<Summary>() < 64);
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
