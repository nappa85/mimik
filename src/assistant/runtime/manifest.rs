//! The pinned llama.cpp build (assisted sheet plan §1.4a, "Pinned build"; ASSIST-33,
//! ASSIST-42): `schemas/llama-runtime-v1.json`, embedded, also read by `llama-fetch` and by
//! packaging, so CI and the binary verify the same set of files.
use super::Path;
use serde::Deserialize;
use std::sync::LazyLock;

/// The manifest's text as shipped (a test asserts it equals the data file).
pub(crate) const TEXT: &str = include_str!("../../../schemas/llama-runtime-v1.json");

#[derive(Debug, Deserialize)]
pub(crate) struct Manifest {
    #[serde(rename = "manifest")]
    pub format: String,
    pub llama_cpp: Llama,
    pub builds: Vec<Build>,
    /// The CI test model (live tests and `llama-fetch` only).
    #[cfg(test)]
    pub test_model: TestModel,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Llama {
    pub tag: String,
    pub commit: String,
    #[cfg(test)]
    pub build: u32,
    /// The first line `llama-server --version` prints (checked by the CI flag check).
    #[cfg(test)]
    pub version_line: String,
}

/// One official release asset and the files Mimik runs from it.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Build {
    /// `cpu` or `vulkan`.
    pub path: String,
    /// The subdirectory of the runtime directory holding exactly `files`.
    pub directory: String,
    /// The server executable's name among `files`.
    pub server: String,
    pub files: Vec<File>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct File {
    pub name: String,
    pub sha256: String,
    pub size: u64,
    /// Octal, as packaging applies it (`0755`, `0644`).
    #[cfg(test)]
    pub mode: String,
}

#[cfg(test)]
#[derive(Debug, Deserialize)]
pub(crate) struct TestModel {
    pub name: String,
    pub sha256: String,
    pub ctx_size: u32,
}

/// The embedded manifest.
pub(crate) static MANIFEST: LazyLock<Manifest> = LazyLock::new(|| {
    let manifest: Manifest =
        serde_json::from_str(TEXT).expect("the embedded runtime manifest is valid");
    assert_eq!(manifest.format, "mimik-llama-runtime-v1");
    manifest
});

impl Manifest {
    /// The build a path runs, when the manifest pins one.
    pub(crate) fn build(&self, path: Path) -> Option<&Build> {
        let name = match path {
            Path::Cpu => "cpu",
            Path::Gpu => "vulkan",
        };
        self.builds.iter().find(|build| build.path == name)
    }
}

#[cfg(test)]
impl File {
    /// The manifest's mode as bits.
    pub(crate) fn mode_bits(&self) -> Option<u32> {
        u32::from_str_radix(self.mode.trim_start_matches('0'), 8).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_manifest_is_the_data_file() {
        let on_disk = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/schemas/llama-runtime-v1.json"
        ))
        .unwrap();
        assert_eq!(TEXT, on_disk);
    }

    #[test]
    fn the_manifest_pins_the_cpu_build_completely() {
        let manifest = &*MANIFEST;
        assert_eq!(manifest.llama_cpp.tag, "b11609");
        assert_eq!(manifest.llama_cpp.build, 11609);
        assert_eq!(manifest.llama_cpp.commit.len(), 40);
        assert!(
            manifest
                .llama_cpp
                .version_line
                .contains(&manifest.llama_cpp.commit[..9])
        );
        let cpu = manifest.build(Path::Cpu).unwrap();
        assert!(cpu.files.iter().any(|file| file.name == cpu.server));
        let mut names = std::collections::BTreeSet::new();
        for file in &cpu.files {
            assert!(names.insert(file.name.as_str()), "{}", file.name);
            assert_eq!(file.sha256.len(), 64);
            assert!(file.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
            assert!(!file.name.contains('/') && !file.name.starts_with('.'));
            let mode = file.mode_bits().unwrap();
            assert_eq!(mode & 0o022, 0, "{}", file.name);
            assert!(file.size > 0);
        }
        // No network backend is shipped.
        assert!(!names.iter().any(|name| name.contains("rpc")));
        assert_eq!(manifest.test_model.sha256.len(), 64);
        assert!(manifest.test_model.ctx_size > 0);
    }

    #[test]
    fn the_gpu_build_is_the_cpu_build_plus_the_vulkan_backend() {
        let manifest = &*MANIFEST;
        let cpu = manifest.build(Path::Cpu).unwrap();
        let gpu = manifest.build(Path::Gpu).unwrap();
        assert_eq!(gpu.directory, "vulkan");
        assert_eq!(gpu.server, cpu.server);
        let extra: Vec<&str> = gpu
            .files
            .iter()
            .filter(|file| {
                !cpu.files
                    .iter()
                    .any(|shared| shared.name == file.name && shared.sha256 == file.sha256)
            })
            .map(|file| file.name.as_str())
            .collect();
        assert_eq!(extra, ["libggml-vulkan.so"]);
        assert_eq!(gpu.files.len(), cpu.files.len() + 1);
        assert!(!gpu.files.iter().any(|file| file.name.contains("rpc")));
    }
}
