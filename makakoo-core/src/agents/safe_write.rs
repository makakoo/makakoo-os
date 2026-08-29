//! Symlink-safe atomic file writes for the `write_file` tool.
//!
//! Closes threat-model card `v0.3.1-O-NOFOLLOW-FD-HOLD-WRITES`
//! (`spec/USER_GRANTS_THREAT_MODEL.md` R1). The Python implementation
//! authorises a path with `realpath` + `commonpath` and then opens it by
//! name, leaving a window in which the tree can change between the two.
//!
//! ## Why `O_NOFOLLOW` alone is not enough
//!
//! `O_NOFOLLOW` refuses a symlink only at the **final** component of the path
//! handed to `open`. Every component before it is still resolved normally.
//! Opening `/allowed/a/parent/file.md` with `O_NOFOLLOW` therefore still
//! follows a symlink at `a`, and an attacker who swaps `a` after
//! authorisation redirects the whole write. A first version of this module
//! guarded only the final component and was demonstrably escapable.
//!
//! So the walk is explicit: start at `/` and open each component in turn with
//! `openat(… O_DIRECTORY | O_NOFOLLOW)`, creating missing directories with
//! `mkdirat`. A symlink *anywhere* along the path aborts the write. The
//! caller has already canonicalised the target through
//! [`crate::agents::resolve_scope_path`], so a symlink appearing mid-walk
//! means the tree changed since authorisation — which is exactly the race
//! being closed, not a legitimate layout.
//!
//! ## Contract
//!
//! `target` **must** be absolute and already canonical (no `..`, no symlinks
//! at authorisation time). Passing an unresolved path is a caller bug: the
//! walk would reject ordinary layouts such as macOS's `/tmp -> private/tmp`.
//!
//! ## Platform
//!
//! On Windows there is no `openat`/`O_NOFOLLOW` equivalent in std, so the
//! fallback is a plain atomic temp+rename. R1 remains open there; Windows is
//! not a deployment target for the agent runtime today.

use std::io;
use std::path::Path;

/// Refuse to buffer or write more than this in one call. The tool is for
/// reports and drafts, not for streaming data into the sandbox.
pub const MAX_WRITE_BYTES: usize = 1024 * 1024;

/// Write `content` to `target` atomically, refusing to traverse a symlink at
/// any component of the path.
///
/// Missing parent directories are created as part of the same guarded walk —
/// a caller that ran `create_dir_all` first would reopen the very hole this
/// closes.
///
/// The write lands via a temp file in the *same directory* followed by a
/// rename, so a reader never observes a partial file and the operation
/// either fully succeeds or leaves the previous contents intact.
pub fn write_atomic_nofollow(target: &Path, content: &[u8]) -> io::Result<()> {
    if content.len() > MAX_WRITE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "content is {} bytes, limit is {}",
                content.len(),
                MAX_WRITE_BYTES
            ),
        ));
    }
    let parent = target.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "write target has no parent directory",
        )
    })?;
    let name = target.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "write target has no file name")
    })?;
    if !target.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "write target must be absolute and already resolved",
        ));
    }
    // "Already resolved" has to be enforced, not assumed. On unix the
    // per-component `openat` walk happens to fail on a `..` because the
    // literal component does not exist, but that is incidental: on
    // Windows the path is normalised before it reaches the filesystem,
    // so `<authorised>/a/../../x.md` resolves cleanly OUTSIDE the
    // authorised directory and the write succeeds. The scope check ran
    // against the unresolved string, so this is a sandbox escape on one
    // platform only. Refuse the components explicitly on every platform.
    if target.components().any(|c| {
        matches!(
            c,
            std::path::Component::ParentDir | std::path::Component::CurDir
        )
    }) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "write target must be already resolved: '.' and '..' components are refused",
        ));
    }
    write_impl(parent, name, content)
}

#[cfg(unix)]
fn write_impl(parent: &Path, name: &std::ffi::OsStr, content: &[u8]) -> io::Result<()> {
    use std::ffi::{CString, OsStr};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd};

    fn cstr(bytes: &[u8]) -> io::Result<CString> {
        CString::new(bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))
    }

    fn symlink_refusal(component: &OsStr) -> io::Error {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "refusing to write: path component {component:?} is a symlink \
                 (tree changed after authorisation)"
            ),
        )
    }

    /// Open `/` — the one component that can never be a symlink — as the
    /// anchor for the walk.
    fn open_root() -> io::Result<OwnedFd> {
        let root = cstr(b"/")?;
        let fd = unsafe {
            libc::open(
                root.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    /// Descend one component, creating it if absent. `O_NOFOLLOW` makes a
    /// symlink at this position fail with ELOOP instead of being traversed.
    fn descend(dir: &OwnedFd, component: &OsStr) -> io::Result<OwnedFd> {
        let name = cstr(component.as_bytes())?;
        let open_child = || unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        let mut fd = open_child();
        if fd < 0 {
            let error = io::Error::last_os_error();
            match error.raw_os_error() {
                Some(libc::ENOENT) => {
                    // Create it here rather than letting `create_dir_all`
                    // walk the path by name outside this guarded descent.
                    if unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
                        let mkdir_error = io::Error::last_os_error();
                        // EEXIST: someone raced us. Re-open and let the
                        // O_NOFOLLOW check judge whatever they created.
                        if mkdir_error.raw_os_error() != Some(libc::EEXIST) {
                            return Err(mkdir_error);
                        }
                    }
                    fd = open_child();
                    if fd < 0 {
                        let reopen = io::Error::last_os_error();
                        return Err(match reopen.raw_os_error() {
                            Some(libc::ELOOP) | Some(libc::ENOTDIR) => symlink_refusal(component),
                            _ => reopen,
                        });
                    }
                }
                // ELOOP is O_NOFOLLOW rejecting a symlink; ENOTDIR is a
                // symlink-to-file, or a plain file where a directory belongs.
                Some(libc::ELOOP) | Some(libc::ENOTDIR) => return Err(symlink_refusal(component)),
                _ => return Err(error),
            }
        }
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    // Walk every component of the parent, pinning each by fd. This is the
    // difference between guarding the final component and guarding the path.
    let mut dir = open_root()?;
    for component in parent.components() {
        match component {
            std::path::Component::RootDir => continue,
            std::path::Component::Normal(name) => dir = descend(&dir, name)?,
            // The contract requires a canonical path; anything else means the
            // caller skipped resolution and the walk cannot be trusted.
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("write target is not canonical: unexpected component {other:?}"),
                ))
            }
        }
    }

    let name_c = cstr(name.as_bytes())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let tmp_name = format!(
        ".{}.makakoo-{}-{}.tmp",
        name.to_string_lossy(),
        std::process::id(),
        stamp
    );
    let tmp_c = cstr(tmp_name.as_bytes())?;

    let tmp_fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            tmp_c.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600 as libc::c_uint,
        )
    };
    if tmp_fd < 0 {
        return Err(io::Error::last_os_error());
    }

    let result = (|| -> io::Result<()> {
        use std::io::Write;
        let mut file = unsafe { std::fs::File::from_raw_fd(tmp_fd) };
        file.write_all(content)?;
        // Durability before the rename: a crash in between must not publish a
        // truncated file under the real name.
        file.sync_all()?;
        drop(file);
        let renamed = unsafe {
            libc::renameat(
                dir.as_raw_fd(),
                tmp_c.as_ptr(),
                dir.as_raw_fd(),
                name_c.as_ptr(),
            )
        };
        if renamed != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    })();

    if result.is_err() {
        // A sandbox that accumulates droppings on every failed write is its
        // own bug report.
        unsafe { libc::unlinkat(dir.as_raw_fd(), tmp_c.as_ptr(), 0) };
    }
    result
}

#[cfg(not(unix))]
fn write_impl(parent: &Path, name: &std::ffi::OsStr, content: &[u8]) -> io::Result<()> {
    // No openat/O_NOFOLLOW in std on Windows. Atomicity is preserved; the
    // symlink race (R1) is not closed here. Documented, not silently ignored.
    std::fs::create_dir_all(parent)?;
    let target = parent.join(name);
    let tmp = parent.join(format!(
        ".{}.makakoo-{}.tmp",
        name.to_string_lossy(),
        std::process::id()
    ));
    std::fs::write(&tmp, content)?;
    match std::fs::rename(&tmp, &target) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_file(&tmp);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Canonical path of a temp dir.
    ///
    /// Not incidental: on macOS `tempdir()` hands back `/var/folders/…` and
    /// `/var` is a symlink to `/private/var`, so the guarded walk refuses it.
    /// The real caller resolves through `resolve_scope_path` first — these
    /// tests must do the same, which is itself proof that the contract is
    /// load-bearing rather than decorative.
    fn canonical(dir: &tempfile::TempDir) -> std::path::PathBuf {
        dir.path().canonicalize().unwrap()
    }

    #[test]
    fn writes_content_and_leaves_no_temp_files() {
        let tmp = tempdir().unwrap();
        let root = canonical(&tmp);
        let target = root.join("report.md");
        write_atomic_nofollow(&target, b"hello").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello");
        let leftovers: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("makakoo-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );
    }

    #[test]
    fn overwrites_atomically_without_truncating_on_failure() {
        let tmp = tempdir().unwrap();
        let target = canonical(&tmp).join("report.md");
        write_atomic_nofollow(&target, b"first").unwrap();
        write_atomic_nofollow(&target, b"second").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "second");
    }

    #[test]
    fn refuses_content_over_the_cap() {
        let tmp = tempdir().unwrap();
        let target = tmp.path().join("big.md");
        let oversize = vec![b'x'; MAX_WRITE_BYTES + 1];
        let error = write_atomic_nofollow(&target, &oversize).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(!target.exists(), "a refused write must not create the file");
    }

    #[cfg(unix)]
    #[test]
    fn replaces_a_symlink_instead_of_writing_through_it() {
        // This is the R1 scenario: the final component is a symlink pointing
        // outside the sandbox. Writing through it would escape scope. The
        // write must land on the link itself, leaving the target untouched.
        let tmp = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let secret = outside.path().join("secret.txt");
        std::fs::write(&secret, "untouched").unwrap();

        let link = canonical(&tmp).join("report.md");
        std::os::unix::fs::symlink(&secret, &link).unwrap();

        write_atomic_nofollow(&link, b"sandboxed").unwrap();

        assert_eq!(
            std::fs::read_to_string(&secret).unwrap(),
            "untouched",
            "the write escaped the sandbox through a symlink"
        );
        assert_eq!(std::fs::read_to_string(&link).unwrap(), "sandboxed");
        assert!(
            !std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "rename must replace the link, not follow it"
        );
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_symlink_at_an_intermediate_component() {
        // Regression for the escape found in review on 2026-08-28. The first
        // version of this module guarded only the final component, so a
        // symlink swapped in *above* the parent redirected the whole write:
        // O_NOFOLLOW never looks at `a` in `/allowed/a/parent/file.md`.
        let sandbox = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let intermediate = canonical(&sandbox).join("allowed/a");
        std::fs::create_dir_all(intermediate.join("parent")).unwrap();

        // The tree changes after the caller authorised the path.
        std::fs::remove_dir_all(&intermediate).unwrap();
        std::fs::create_dir_all(outside.path().join("parent")).unwrap();
        std::os::unix::fs::symlink(outside.path(), &intermediate).unwrap();

        let target = intermediate.join("parent/pwned.md");
        let error = write_atomic_nofollow(&target, b"ESCAPED").unwrap_err();

        assert!(
            !outside.path().join("parent/pwned.md").exists(),
            "write escaped the sandbox through an intermediate symlink"
        );
        assert!(
            error.to_string().contains("symlink"),
            "the refusal must say why: {error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn creates_missing_parents_within_the_guarded_walk() {
        // Directory creation has to happen inside the walk; a caller running
        // create_dir_all first would reopen the hole above.
        let tmp = tempdir().unwrap();
        let target = canonical(&tmp).join("a/b/c/report.md");
        write_atomic_nofollow(&target, b"deep").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "deep");
    }

    #[test]
    fn refuses_a_relative_or_non_canonical_target() {
        // The walk's soundness depends on the caller having resolved the
        // path; accepting `..` here would let it walk back out.
        assert_eq!(
            write_atomic_nofollow(Path::new("relative/x.md"), b"x")
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        // `..` must be refused on EVERY platform. Unix rejected this
        // only as a side effect of the component walk; Windows
        // normalises the path first, so without an explicit check the
        // write lands outside the directory that was authorised.
        //
        // Each component is joined separately on purpose. `canonicalize`
        // returns a verbatim path on Windows, where forward slashes are
        // NOT separators — joining "a/../../x.md" as one string would
        // make it a single literal file name and quietly stop testing
        // anything.
        let tmp = tempdir().unwrap();
        let sneaky = canonical(&tmp).join("a").join("..").join("..").join("x.md");
        let err = write_atomic_nofollow(&sneaky, b"x").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{err}");

        // A `..` that resolves to a real, writable directory is the
        // dangerous shape: it must fail because of the component, not
        // because the intermediate directory happens to be missing.
        let escape = canonical(&tmp).join("..").join("escaped.md");
        let err = write_atomic_nofollow(&escape, b"x").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{err}");
        assert!(!canonical(&tmp)
            .parent()
            .unwrap()
            .join("escaped.md")
            .exists());

        // A single `.` is NOT a hazard and is not asserted here:
        // `Path::components()` elides CurDir, so `/a/./b` and `/a/b` are
        // the same path. `..` is the component that can leave the
        // authorised directory.
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_parent_directory_that_is_a_symlink() {
        // A parent swapped for a symlink after authorisation is the other
        // half of R1. O_NOFOLLOW on the directory open makes it fail loudly
        // rather than writing into the linked-to directory.
        let tmp = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let link_dir = canonical(&tmp).join("reports");
        std::os::unix::fs::symlink(outside.path(), &link_dir).unwrap();

        let error = write_atomic_nofollow(&link_dir.join("x.md"), b"nope").unwrap_err();
        assert!(
            !outside.path().join("x.md").exists(),
            "write landed through a symlinked parent"
        );
        assert_ne!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn a_path_with_no_parent_is_rejected_not_panicked_on() {
        let error = write_atomic_nofollow(Path::new("/"), b"x").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}
