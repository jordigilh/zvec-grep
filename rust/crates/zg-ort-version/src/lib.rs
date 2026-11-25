//! Version of the ONNX Runtime linked into this binary.
//!
//! `ort` (2.0.0-rc.13) wraps `OrtGetApiBase().GetVersionString` privately, but
//! the version of the ORT build that produces embedding vectors is part of
//! zvec-grep's backend identity: two runtimes with the same execution provider
//! and dtype but different ORT builds must not serve each other's indexes.
//! This crate quarantines the unsafe C call behind a safe, cached accessor.

use std::ffi::CStr;
use std::sync::OnceLock;

/// Version of the linked ONNX Runtime, as reported by
/// `OrtGetApiBase().GetVersionString` (e.g. `1.22.0`), or `unknown` when the
/// runtime does not report one. Cached once: the buffer is owned by the
/// runtime and never changes for one process.
pub fn onnx_runtime_version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(|| {
        // SAFETY: `OrtGetApiBase` is the linked runtime's own exported entry
        // point (this build links ORT without `load-dynamic`), so the returned
        // base pointer is valid, and the C API places no initialization
        // requirement on `OrtApiBase` functions. The version buffer is owned
        // by ORT and must not be freed; we only copy it out.
        let version = unsafe {
            let base = ort::sys::OrtGetApiBase();
            if base.is_null() {
                None
            } else {
                let version = ((*base).GetVersionString)();
                if version.is_null() {
                    None
                } else {
                    Some(CStr::from_ptr(version).to_string_lossy().into_owned())
                }
            }
        };
        match version {
            Some(version) if !version.is_empty() => version,
            // Identity paths must not panic, and a null or empty version
            // string is not a documented ORT failure mode; record it visibly
            // so a binary embedding `ort-unknown` is diagnosable.
            _ => {
                tracing::warn!(
                    "ONNX Runtime did not report a version string; \
                     recording the ONNX Runtime version as 'unknown'"
                );
                "unknown".to_owned()
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::onnx_runtime_version;

    #[test]
    fn reports_a_stable_non_empty_version() {
        let version = onnx_runtime_version();
        assert!(!version.is_empty(), "ORT must report a version string");
        assert_eq!(onnx_runtime_version(), version);
    }
}
