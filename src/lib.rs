//! rFPM — build deb, rpm, and Arch Linux packages from Rust.
//!
//! ```no_run
//! use std::fs::File;
//! use rfpm::{Package, Arch, FileOptions};
//!
//! let mut pkg = Package::new("myapp", "1.0.0", Arch::Amd64, "My application");
//! pkg.add_file_with(
//!     "/usr/bin/myapp",
//!     File::open("target/release/myapp").unwrap(),
//!     FileOptions { mode: 0o755, ..Default::default() },
//! );
//! pkg.add_config("/etc/myapp/config.toml", "# default config\n".to_string());
//! pkg.depends.push("libc6".into());
//!
//! let mut out = File::create(pkg.deb_filename()).unwrap();
//! pkg.write_deb(&mut out).unwrap();
//! ```

mod arch;
mod content;
mod distro;

pub use arch::Arch;
pub use content::Content;

use std::collections::HashMap;
use std::fmt;

/// Error type for rFPM operations.
#[derive(Debug)]
pub enum Error {
    /// An I/O error occurred while reading content or writing the package.
    Io(std::io::Error),
    /// The package name contains invalid characters.
    InvalidName(String),
    /// A required field is missing.
    MissingField(&'static str),
    /// An error from the underlying RPM library.
    Rpm(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "I/O error: {e}"),
            Error::InvalidName(name) => write!(f, "invalid package name: {name}"),
            Error::MissingField(field) => write!(f, "missing required field: {field}"),
            Error::Rpm(msg) => write!(f, "RPM error: {msg}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

// --- Package ---

/// A Linux package definition.
///
/// Contains all metadata, files, and scripts needed to build a `.deb`,
/// `.rpm`, or `.pkg.tar.zst` package. Format-specific options are set
/// via the [`deb`](Package::deb), [`rpm`](Package::rpm), and
/// [`arch_linux`](Package::arch_linux) fields.
pub struct Package {
    /// Package name (e.g. "myapp").
    pub name: String,
    /// Upstream version (e.g. "1.0.0").
    pub version: String,
    /// Package release/revision number. Defaults to `"1"`.
    pub release: String,
    /// Version epoch for ordering (e.g. `Some(2)` means `2:1.0.0`).
    pub epoch: Option<u32>,
    /// Target CPU architecture.
    pub arch: Arch,
    /// Package description.
    pub description: String,
    /// Homepage URL.
    pub homepage: Option<String>,
    /// License identifier (e.g. "MIT", "GPL-3.0").
    pub license: Option<String>,
    /// Package maintainer (e.g. "Name <email>").
    pub maintainer: Option<String>,
    /// Organization that distributes the software.
    pub vendor: Option<String>,

    /// Packages this package depends on at runtime.
    pub depends: Vec<String>,
    /// Virtual packages this package provides.
    pub provides: Vec<String>,
    /// Packages this package conflicts with.
    pub conflicts: Vec<String>,
    /// Packages this package replaces/obsoletes.
    pub replaces: Vec<String>,
    /// Recommended (but not required) packages.
    pub recommends: Vec<String>,
    /// Suggested packages.
    pub suggests: Vec<String>,

    /// Shared lifecycle scripts (pre/post install/remove).
    pub scripts: Scripts,

    /// Deb-specific options.
    pub deb: DebOptions,
    /// RPM-specific options.
    pub rpm: RpmOptions,
    /// Arch Linux-specific options.
    pub arch_linux: ArchOptions,

    /// File/directory/symlink entries in the package.
    pub(crate) entries: Vec<Entry>,
}

impl Package {
    /// Create a new package with required fields.
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
        arch: Arch,
        description: impl Into<String>,
    ) -> Self {
        Package {
            name: name.into(),
            version: version.into(),
            release: "1".into(),
            epoch: None,
            arch,
            description: description.into(),
            homepage: None,
            license: None,
            maintainer: None,
            vendor: None,
            depends: Vec::new(),
            provides: Vec::new(),
            conflicts: Vec::new(),
            replaces: Vec::new(),
            recommends: Vec::new(),
            suggests: Vec::new(),
            scripts: Scripts::default(),
            deb: DebOptions::default(),
            rpm: RpmOptions::default(),
            arch_linux: ArchOptions::default(),
            entries: Vec::new(),
        }
    }

    /// Add a regular file to the package with default mode (0o644) and root ownership.
    ///
    /// `dest` is the absolute path inside the package (e.g. `"/usr/share/myapp/data.txt"`).
    /// `source` is the file content — accepts [`File`](std::fs::File), [`Vec<u8>`],
    /// [`String`], [`Cursor`](std::io::Cursor), or any `Read + Seek` type.
    pub fn add_file(&mut self, dest: impl Into<String>, source: impl Into<Content>) {
        self.add_file_with(dest, source, FileOptions::default());
    }

    /// Add a regular file with explicit [`FileOptions`] (mode, owner, group).
    ///
    /// ```
    /// # use rfpm::{Package, Arch, FileOptions};
    /// # let mut pkg = Package::new("x", "1", Arch::Amd64, "x");
    /// // Executable owned by root:
    /// pkg.add_file_with("/usr/bin/x", "data".to_string(), FileOptions { mode: 0o755, ..Default::default() });
    ///
    /// // File with custom ownership:
    /// pkg.add_file_with("/etc/app/cfg", "data".to_string(), FileOptions {
    ///     owner: "appuser".into(),
    ///     group: "appgroup".into(),
    ///     ..Default::default()
    /// });
    /// ```
    pub fn add_file_with(
        &mut self,
        dest: impl Into<String>,
        source: impl Into<Content>,
        opts: FileOptions,
    ) {
        self.push_file(dest, source, opts, false);
    }

    /// Add a configuration file that package managers treat specially.
    ///
    /// - **deb**: listed in `conffiles` — dpkg won't overwrite user edits on upgrade
    /// - **rpm**: gets the `ConfigFile` flag — rpm prompts about conflicts during upgrade
    /// - **arch**: listed in the `backup` field of `.PKGINFO`
    pub fn add_config(&mut self, dest: impl Into<String>, source: impl Into<Content>) {
        self.push_file(dest, source, FileOptions::default(), true);
    }

    /// Add an explicit empty directory with default mode (0o755).
    ///
    /// Directories are created implicitly as parents of files, but this
    /// lets you create standalone directories (e.g. `"/var/lib/myapp"`).
    pub fn add_dir(&mut self, dest: impl Into<String>) {
        self.push_dir(dest, 0o755);
    }

    /// Add an explicit empty directory with specific Unix permissions.
    pub fn add_dir_with_mode(&mut self, dest: impl Into<String>, mode: u32) {
        self.push_dir(dest, mode);
    }

    fn push_file(
        &mut self,
        dest: impl Into<String>,
        source: impl Into<Content>,
        opts: FileOptions,
        is_config: bool,
    ) {
        self.entries.push(Entry {
            dest: dest.into(),
            kind: EntryKind::File {
                source: source.into(),
                is_config,
            },
            mode: opts.mode,
            owner: opts.owner,
            group: opts.group,
        });
    }

    fn push_dir(&mut self, dest: impl Into<String>, mode: u32) {
        self.entries.push(Entry {
            dest: dest.into(),
            kind: EntryKind::Directory,
            mode,
            owner: "root".into(),
            group: "root".into(),
        });
    }

    /// Add a symbolic link.
    ///
    /// `dest` is the path where the symlink is created.
    /// `target` is what it points to.
    ///
    /// For example, `add_symlink("/usr/bin/foo", "/usr/bin/bar")` creates
    /// a symlink at `/usr/bin/foo` pointing to `/usr/bin/bar`.
    pub fn add_symlink(&mut self, dest: impl Into<String>, target: impl Into<String>) {
        self.entries.push(Entry {
            dest: dest.into(),
            kind: EntryKind::Symlink {
                target: target.into(),
            },
            mode: 0o777,
            owner: "root".into(),
            group: "root".into(),
        });
    }

    /// Returns the conventional `.deb` filename.
    ///
    /// Format: `{name}_{version}-{release}_{arch}.deb`
    /// (e.g. `"myapp_1.0.0-1_amd64.deb"`).
    pub fn deb_filename(&self) -> String {
        format!(
            "{}_{}-{}_{}.deb",
            self.name,
            self.version,
            self.release,
            self.arch.to_deb(),
        )
    }

    /// Returns the conventional `.rpm` filename.
    ///
    /// Format: `{name}-{version}-{release}.{arch}.rpm`
    /// (e.g. `"myapp-1.0.0-1.x86_64.rpm"`).
    pub fn rpm_filename(&self) -> String {
        format!(
            "{}-{}-{}.{}.rpm",
            self.name,
            self.version,
            self.release,
            self.arch.to_rpm(),
        )
    }

    /// Returns the conventional Arch Linux `.pkg.tar.zst` filename.
    ///
    /// Format: `{name}-{version}-{release}-{arch}.pkg.tar.zst`
    /// (e.g. `"myapp-1.0.0-1-x86_64.pkg.tar.zst"`).
    pub fn arch_filename(&self) -> String {
        format!(
            "{}-{}-{}-{}.pkg.tar.zst",
            self.name,
            self.version,
            self.release,
            self.arch.to_arch_linux(),
        )
    }
}

// --- FileOptions ---

/// Options for file entries: permissions and ownership.
///
/// Use with [`Package::add_file_with`] to control mode and ownership.
/// Fields default to `0o644` / `root:root` via [`Default`].
///
/// ```
/// use rfpm::FileOptions;
///
/// let opts = FileOptions { mode: 0o755, ..Default::default() };
/// ```
pub struct FileOptions {
    /// Unix file mode (e.g. `0o755`). Defaults to `0o644`.
    pub mode: u32,
    /// Owner username. Defaults to `"root"`.
    pub owner: String,
    /// Group name. Defaults to `"root"`.
    pub group: String,
}

impl Default for FileOptions {
    fn default() -> Self {
        Self {
            mode: 0o644,
            owner: "root".into(),
            group: "root".into(),
        }
    }
}

// --- Entry ---

pub(crate) struct Entry {
    pub dest: String,
    pub kind: EntryKind,
    pub mode: u32,
    pub owner: String,
    pub group: String,
}

pub(crate) enum EntryKind {
    File { source: Content, is_config: bool },
    Directory,
    Symlink { target: String },
}

// --- Scripts ---

/// Shared lifecycle scripts, used by all package formats.
///
/// These four scripts are supported by deb, rpm, and arch alike.
/// Format-specific *additional* scripts (not overrides) are available via
/// [`DebScripts`], [`RpmScripts`], and [`ArchScripts`] on the
/// corresponding options struct.
#[derive(Default)]
pub struct Scripts {
    /// Runs before package files are installed.
    pub pre_install: Option<Content>,
    /// Runs after package files are installed.
    pub post_install: Option<Content>,
    /// Runs before package files are removed.
    pub pre_remove: Option<Content>,
    /// Runs after package files are removed.
    pub post_remove: Option<Content>,
}

// --- Deb ---

/// Deb-specific package options.
pub struct DebOptions {
    /// Package section (e.g. "utils", "net").
    pub section: Option<String>,
    /// Package priority. Defaults to `"optional"`.
    pub priority: String,
    /// Compression for `data.tar.*`. Defaults to [`DebCompression::Gzip`].
    pub compression: DebCompression,
    /// Pre-dependency packages (stronger than `depends`).
    pub predepends: Vec<String>,
    /// Packages that this package breaks.
    pub breaks: Vec<String>,
    /// dpkg trigger directives.
    pub triggers: DebTriggers,
    /// Custom fields added to the `control` file.
    pub fields: HashMap<String, String>,
    /// Deb-specific scripts (rules, templates, config).
    pub scripts: DebScripts,
}

impl Default for DebOptions {
    fn default() -> Self {
        DebOptions {
            section: None,
            priority: "optional".into(),
            compression: DebCompression::Gzip,
            predepends: Vec::new(),
            breaks: Vec::new(),
            triggers: DebTriggers::default(),
            fields: HashMap::new(),
            scripts: DebScripts::default(),
        }
    }
}

/// Compression algorithm for deb `data.tar.*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DebCompression {
    #[default]
    Gzip,
    Xz,
    Zstd,
    None,
}

/// dpkg trigger directives.
///
/// See <https://man7.org/linux/man-pages/man5/deb-triggers.5.html>.
#[derive(Default)]
pub struct DebTriggers {
    pub interest: Vec<String>,
    pub interest_await: Vec<String>,
    pub interest_noawait: Vec<String>,
    pub activate: Vec<String>,
    pub activate_await: Vec<String>,
    pub activate_noawait: Vec<String>,
}

/// Deb-specific scripts included in `control.tar.gz`.
///
/// These are *in addition to* the shared [`Scripts`] (pre/post install/remove),
/// which are also written into the deb's control archive.
#[derive(Default)]
pub struct DebScripts {
    /// `debian/rules` file.
    pub rules: Option<Content>,
    /// debconf templates file.
    pub templates: Option<Content>,
    /// debconf config script.
    pub config: Option<Content>,
}

// --- RPM ---

/// RPM-specific package options.
pub struct RpmOptions {
    /// Short one-line summary. Defaults to the first line of `description`.
    pub summary: Option<String>,
    /// RPM package group (e.g. "Applications/System").
    pub group: Option<String>,
    /// Build host name. Defaults to the system hostname.
    pub build_host: Option<String>,
    /// Organization that packaged the software.
    pub packager: Option<String>,
    /// Compression algorithm. Defaults to [`RpmCompression::Gzip`].
    pub compression: RpmCompression,
    /// Prefixes for relocatable packages.
    pub prefixes: Vec<String>,
    /// RPM-specific scripts (pretrans, posttrans, verify).
    pub scripts: RpmScripts,
    /// Destination paths for ghost files (tracked by RPM but not installed).
    pub ghost_files: Vec<String>,
}

impl Default for RpmOptions {
    fn default() -> Self {
        RpmOptions {
            summary: None,
            group: None,
            build_host: None,
            packager: None,
            compression: RpmCompression::Gzip,
            prefixes: Vec::new(),
            scripts: RpmScripts::default(),
            ghost_files: Vec::new(),
        }
    }
}

/// Compression algorithm for RPM packages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RpmCompression {
    #[default]
    Gzip,
    Xz,
    Zstd,
    Lzma,
}

/// RPM-specific lifecycle scripts.
///
/// These are *in addition to* the shared [`Scripts`] (pre/post install/remove).
/// Transaction scripts bracket the entire RPM transaction, while the shared
/// scripts run per-package within it.
#[derive(Default)]
pub struct RpmScripts {
    /// Runs before the RPM transaction begins (before any pre_install).
    pub pre_trans: Option<Content>,
    /// Runs after the RPM transaction completes (after any post_install).
    pub post_trans: Option<Content>,
    /// Runs when `rpm -V` verifies the package.
    pub verify: Option<Content>,
}

// --- Arch Linux ---

/// Arch Linux-specific package options.
#[derive(Default)]
pub struct ArchOptions {
    /// Base package name for split packages. Defaults to the package name.
    pub pkgbase: Option<String>,
    /// Packager identity string. Defaults to `"Unknown Packager"`.
    pub packager: Option<String>,
    /// Arch-specific scripts (pre/post upgrade).
    pub scripts: ArchScripts,
}

/// Arch Linux-specific lifecycle scripts.
///
/// These are *in addition to* the shared [`Scripts`] (pre/post install/remove).
/// Arch Linux distinguishes between a fresh install and an upgrade — the shared
/// scripts handle installs/removes, while these handle upgrades specifically.
/// All are written into the `.INSTALL` file as shell functions.
#[derive(Default)]
pub struct ArchScripts {
    /// Runs before an upgrade (not a fresh install).
    pub pre_upgrade: Option<Content>,
    /// Runs after an upgrade (not a fresh install).
    pub post_upgrade: Option<Content>,
}
