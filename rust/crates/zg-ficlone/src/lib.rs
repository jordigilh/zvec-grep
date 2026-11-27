//! Linux reflink (`FICLONE`) copy for seeding workspace indexes.
//!
//! `rustix` exposes no `FICLONE` wrapper in the pinned version and the
//! standard library has none, so this crate quarantines the single unsafe
//! ioctl the same way `zg-ort-version` quarantines the ORT version call;
//! every other crate keeps the workspace `unsafe_code = "forbid"`.

#[cfg(target_os = "linux")]
mod linux {
    use std::{fs::File, io, os::fd::AsRawFd};

    /// The generic (x86-64/aarch64) request encoding of `FICLONE`. Exotic
    /// architectures encode ioctl request bits differently; there the call
    /// fails and the caller degrades to a full copy.
    const FICLONE: libc::c_ulong = 0x40049409;

    /// Clone `source`'s extents into `destination` copy-on-write. The
    /// destination must have just been created and not yet written. The
    /// ioctl does not carry the source's mode bits; the caller applies them.
    ///
    /// Returns the operating system's error when the filesystem does not
    /// support reflinks (for example ext4), so the caller can fall back to a
    /// full copy.
    pub fn ficlone(destination: &File, source: &File) -> io::Result<()> {
        let result = unsafe { libc::ioctl(destination.as_raw_fd(), FICLONE, source.as_raw_fd()) };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::ficlone;
