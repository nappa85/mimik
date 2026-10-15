//! Development-only JSON-lines bridge to `mimik::definitions::formula` for Python prototypes.
//!
//! Reads one JSON request per line from stdin and writes one JSON response per line to stdout.
//! It opens no files, databases or network connections. Requests:
//! `{"op":"version"}`, `{"op":"compile","sheet":{...}}`, `{"op":"evaluate","values":{...}}`.
//! Excluded from the corresponding-source package: it is not part of the shipped application.
use mimik::definitions::formula::{Compiled, PROFILE, SheetSpec};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::{BufRead, Read, Write};

const MAX_LINE_BYTES: u64 = 4 * 1024 * 1024;
const FIXTURE: &[u8] = include_bytes!("../schemas/formula-conformance-v1.json");

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Version,
    Compile { sheet: SheetSpec },
    Evaluate { values: Value },
}

fn respond(compiled: &mut Option<Compiled>, line: &str) -> Value {
    let Ok(request) = serde_json::from_str::<Request>(line) else {
        return json!({"error": "bad_request"});
    };
    match request {
        Request::Version => json!({"ok": {
            "profile": PROFILE,
            "engine": "cel 0.15.0",
            "fixture_sha256": hex::encode(Sha256::digest(FIXTURE)),
        }}),
        Request::Compile { sheet } => match Compiled::new(&sheet) {
            Ok(program) => {
                let response = json!({"ok": {
                    "derived_order": program.derived_order(),
                    "cost": program.cost(),
                }});
                *compiled = Some(program);
                response
            }
            Err(rejection) => {
                *compiled = None;
                json!({"reject": rejection})
            }
        },
        Request::Evaluate { values } => match compiled.as_ref() {
            None => json!({"error": "not_compiled"}),
            Some(program) => match program.evaluate(&values) {
                Ok(evaluation) => json!({"ok": evaluation.to_json()}),
                Err(error) => json!({"input_error": error}),
            },
        },
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let mut output = std::io::stdout().lock();
    let mut compiled = None;
    loop {
        let mut line = String::new();
        let read = (&mut input).take(MAX_LINE_BYTES).read_line(&mut line)?;
        if read == 0 {
            return Ok(());
        }
        let response = if line.ends_with('\n') {
            respond(&mut compiled, line.trim_end())
        } else if u64::try_from(read)? >= MAX_LINE_BYTES {
            return Err("request line exceeds limit".into());
        } else {
            respond(&mut compiled, &line)
        };
        serde_json::to_writer(&mut output, &response)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
}
