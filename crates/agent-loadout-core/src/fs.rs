//! File system facade. Core code takes `&dyn Fs` so tests can confine every
//! access to a temp root (`agent_loadout_testkit::RootedFs`).

use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub trait Fs: Send + Sync {
    /// The directory this view is confined to, or `None` for the whole file
    /// system. Ancestor walks (such as sync-root detection) stop here.
    fn root(&self) -> Option<&Path>;

    fn exists(&self, path: &Path) -> bool;

    fn is_dir(&self, path: &Path) -> bool;

    fn read_to_string(&self, path: &Path) -> io::Result<String>;

    /// The bytes of a file.
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;

    fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()>;

    /// Creates `path`, which must not exist, writes `contents` and flushes
    /// them to disk. With `like`, the new file gets the permissions of that
    /// existing file: its mode on Unix, its attributes and DACL on Windows.
    /// Used for the temp file of an atomic replace (SPEC 8.4).
    fn write_new(&self, path: &Path, contents: &[u8], like: Option<&Path>) -> io::Result<()>;

    /// Moves `from` over `to`, replacing it atomically. On Unix the
    /// directory is flushed afterwards.
    fn replace(&self, from: &Path, to: &Path) -> io::Result<()>;

    /// Writes `contents` to `path` (created or truncated) so that only the
    /// current user can read or write it: mode 0600 on Unix, a protected
    /// DACL granting only the file's owner on Windows. Flushes to disk.
    fn write_private(&self, path: &Path, contents: &[u8]) -> io::Result<()>;

    fn create_dir_all(&self, path: &Path) -> io::Result<()>;

    /// Creates `path` and its missing parents; `path` itself is restricted
    /// to the current user (mode 0700 on Unix, a protected DACL granting
    /// only the owner, inherited by new children, on Windows).
    fn create_private_dir_all(&self, path: &Path) -> io::Result<()>;

    fn remove_file(&self, path: &Path) -> io::Result<()>;

    fn remove_dir_all(&self, path: &Path) -> io::Result<()>;

    /// Paths of the entries of a directory, sorted.
    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>>;

    /// Opens `path` for use as an advisory lock file, creating it if needed.
    /// The contents are never read or written.
    fn open_lock_file(&self, path: &Path) -> io::Result<File>;

    /// `path` with symlinks and junctions resolved (`std::fs::canonicalize`;
    /// on Windows a `\\?\` path). Views that cannot resolve links return
    /// `path` as it is.
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        Ok(path.to_path_buf())
    }
}

/// The real file system, unconfined.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealFs;

impl Fs for RealFs {
    fn root(&self) -> Option<&Path> {
        None
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        std::fs::read_to_string(path)
    }

    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        std::fs::write(path, contents)
    }

    fn write_new(&self, path: &Path, contents: &[u8], like: Option<&Path>) -> io::Result<()> {
        let mut options = File::options();
        options.write(true).create_new(true);
        if let Some(like) = like {
            platform::copy_open_options(&mut options, like)?;
        }
        let mut file = options.open(path)?;
        let written = (|| {
            if let Some(like) = like {
                platform::copy_permissions(like, path, &file)?;
            }
            file.write_all(contents)?;
            file.sync_all()
        })();
        if written.is_err() {
            drop(file);
            let _ = std::fs::remove_file(path);
        }
        written
    }

    fn replace(&self, from: &Path, to: &Path) -> io::Result<()> {
        std::fs::rename(from, to)?;
        platform::sync_parent(to)
    }

    fn write_private(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        let mut options = File::options();
        options.write(true).create(true).truncate(true);
        platform::private_open_options(&mut options);
        let mut file = options.open(path)?;
        // Restrict before any content lands, also for an existing file.
        platform::make_private(path, &file)?;
        file.write_all(contents)?;
        file.sync_all()
    }

    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        std::fs::create_dir_all(path)
    }

    fn create_private_dir_all(&self, path: &Path) -> io::Result<()> {
        std::fs::create_dir_all(path)?;
        platform::make_private_dir(path)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        std::fs::remove_file(path)
    }

    fn remove_dir_all(&self, path: &Path) -> io::Result<()> {
        std::fs::remove_dir_all(path)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        let mut entries = std::fs::read_dir(path)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<io::Result<Vec<_>>>()?;
        entries.sort();
        Ok(entries)
    }

    fn open_lock_file(&self, path: &Path) -> io::Result<File> {
        File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
    }

    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        std::fs::canonicalize(path)
    }
}

#[cfg(unix)]
mod platform {
    use std::fs::{File, OpenOptions, Permissions};
    use std::io;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    use std::path::Path;

    pub fn copy_open_options(options: &mut OpenOptions, like: &Path) -> io::Result<()> {
        let mode = std::fs::metadata(like)?.permissions().mode();
        options.mode(mode & 0o7777);
        Ok(())
    }

    /// The umask may have narrowed the mode given at creation.
    pub fn copy_permissions(like: &Path, _path: &Path, file: &File) -> io::Result<()> {
        let mode = std::fs::metadata(like)?.permissions().mode();
        file.set_permissions(Permissions::from_mode(mode & 0o7777))
    }

    pub fn private_open_options(options: &mut OpenOptions) {
        options.mode(0o600);
    }

    pub fn make_private(_path: &Path, file: &File) -> io::Result<()> {
        file.set_permissions(Permissions::from_mode(0o600))
    }

    pub fn make_private_dir(path: &Path) -> io::Result<()> {
        std::fs::set_permissions(path, Permissions::from_mode(0o700))
    }

    /// Flushes the directory entry of a rename.
    pub fn sync_parent(path: &Path) -> io::Result<()> {
        match path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            Some(dir) => File::open(dir)?.sync_all(),
            None => Ok(()),
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::fs::{File, OpenOptions};
    use std::io;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::path::Path;

    use windows_permissions::constants::{AceFlags, SeObjectType, SecurityInformation};
    use windows_permissions::{LocalBox, SecurityDescriptor, wrappers};

    /// Attributes a replaced file keeps (as `ReplaceFileW` does). Read-only
    /// is not carried over: the replace itself would fail on it.
    const KEPT_ATTRIBUTES: u32 = 0x2 // hidden
        | 0x4 // system
        | 0x20 // archive
        | 0x2000; // not content indexed

    pub fn copy_open_options(options: &mut OpenOptions, like: &Path) -> io::Result<()> {
        let attributes = std::fs::metadata(like)?.file_attributes() & KEPT_ATTRIBUTES;
        options.attributes(attributes);
        Ok(())
    }

    /// Gives `path` the DACL of `like`. A DACL made only of inherited
    /// entries is already right: the new file sits in the same directory.
    pub fn copy_permissions(like: &Path, path: &Path, _file: &File) -> io::Result<()> {
        let sd = wrappers::GetNamedSecurityInfo(
            like.as_os_str(),
            SeObjectType::SE_FILE_OBJECT,
            SecurityInformation::Dacl,
        )?;
        let sddl = sd.as_sddl()?.to_string_lossy().into_owned();
        let protected = dacl_flags(&sddl).contains('P');
        let Some(dacl) = sd.dacl() else {
            return Ok(());
        };
        let explicit = (0..dacl.len())
            .filter_map(|index| dacl.get_ace(index))
            .any(|ace| !ace.flags().contains(AceFlags::Inherited));
        if !protected && !explicit {
            return Ok(());
        }
        let protection = if protected {
            SecurityInformation::ProtectedDacl
        } else {
            SecurityInformation::UnprotectedDacl
        };
        wrappers::SetNamedSecurityInfo(
            path.as_os_str(),
            SeObjectType::SE_FILE_OBJECT,
            SecurityInformation::Dacl | protection,
            None,
            None,
            Some(dacl),
            None,
        )
    }

    /// The control flags of the DACL in an SDDL string (`D:PAI(...)` gives
    /// `P` plus `AI`), without the auto-inherit flags.
    fn dacl_flags(sddl: &str) -> String {
        let Some(start) = sddl.find("D:") else {
            return String::new();
        };
        let rest = &sddl[start + 2..];
        let end = rest.find('(').unwrap_or(rest.len());
        rest[..end].replace("AI", "").replace("AR", "")
    }

    pub fn private_open_options(_options: &mut OpenOptions) {}

    pub fn make_private(path: &Path, _file: &File) -> io::Result<()> {
        owner_only(path, false)
    }

    pub fn make_private_dir(path: &Path) -> io::Result<()> {
        owner_only(path, true)
    }

    /// A protected DACL with one entry: full control for the owner of
    /// `path` (the current user for anything this process creates).
    fn owner_only(path: &Path, dir: bool) -> io::Result<()> {
        let sd = wrappers::GetNamedSecurityInfo(
            path.as_os_str(),
            SeObjectType::SE_FILE_OBJECT,
            SecurityInformation::Owner,
        )?;
        let owner = sd
            .owner()
            .ok_or_else(|| io::Error::other("the file has no owner"))?;
        let inherit = if dir { "OICI" } else { "" };
        let sddl = format!("D:P(A;{inherit};FA;;;{owner})");
        let private: LocalBox<SecurityDescriptor> = sddl.parse()?;
        let dacl = private
            .dacl()
            .ok_or_else(|| io::Error::other("the private DACL did not parse"))?;
        wrappers::SetNamedSecurityInfo(
            path.as_os_str(),
            SeObjectType::SE_FILE_OBJECT,
            SecurityInformation::Dacl | SecurityInformation::ProtectedDacl,
            None,
            None,
            Some(dacl),
            None,
        )
    }

    /// NTFS makes a completed rename durable without a directory flush.
    pub fn sync_parent(_path: &Path) -> io::Result<()> {
        Ok(())
    }
}
