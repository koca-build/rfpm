//! RPM package writer.
//!
//! Delegates to the `rpm` crate for binary RPM format construction.

use std::io::Write;

use crate::{EntryKind, Error, Package, RpmCompression};

impl Package {
    /// Write an `.rpm` package to the given writer.
    ///
    /// Delegates to the `rpm` crate for binary RPM format construction.
    /// Maps rFPM types to rpmpack metadata, files, and scripts.
    pub fn write_rpm(&mut self, w: &mut dyn Write) -> Result<(), Error> {
        let summary = self
            .rpm
            .summary
            .clone()
            .unwrap_or_else(|| self.description.lines().next().unwrap_or("").to_string());

        let license = self.license.as_deref().unwrap_or("Unknown");

        let mut builder = rpm::PackageBuilder::new(
            &self.name,
            &self.rpm_version_string(),
            license,
            self.arch.to_rpm(),
            &summary,
        )
        .description(&self.description)
        .release(&self.release);

        // Epoch
        if let Some(epoch) = self.epoch {
            builder = builder.epoch(epoch);
        }

        // Optional metadata
        if let Some(ref hp) = self.homepage {
            builder = builder.url(hp);
        }
        if let Some(ref vendor) = self.vendor {
            builder = builder.vendor(vendor);
        }
        if let Some(ref group) = self.rpm.group {
            builder = builder.group(group);
        }
        if let Some(ref packager) = self.rpm.packager {
            builder = builder.packager(packager);
        } else if let Some(ref maintainer) = self.maintainer {
            builder = builder.packager(maintainer);
        }
        if let Some(ref build_host) = self.rpm.build_host {
            builder = builder.build_host(build_host);
        }

        // Compression
        builder = builder.compression(match self.rpm.compression {
            RpmCompression::Gzip => rpm::CompressionWithLevel::Gzip(9),
            RpmCompression::Xz => rpm::CompressionWithLevel::Xz(9),
            RpmCompression::Zstd => rpm::CompressionWithLevel::Zstd(19),
            RpmCompression::Lzma => rpm::CompressionWithLevel::Gzip(9), // fallback
        });

        // Dependencies
        for dep in &self.depends {
            builder = builder.requires(parse_rpm_dep(dep));
        }
        for dep in &self.provides {
            builder = builder.provides(parse_rpm_dep(dep));
        }
        for dep in &self.conflicts {
            builder = builder.conflicts(parse_rpm_dep(dep));
        }
        for dep in &self.replaces {
            builder = builder.obsoletes(parse_rpm_dep(dep));
        }
        for dep in &self.recommends {
            builder = builder.recommends(parse_rpm_dep(dep));
        }
        for dep in &self.suggests {
            builder = builder.suggests(parse_rpm_dep(dep));
        }

        // Files
        for entry in &mut self.entries {
            match &mut entry.kind {
                EntryKind::File { source, is_config } => {
                    let data = source.read_all()?;
                    let mode = mode_u16(entry.mode)?;
                    let tmp = temp_file(&data)?;

                    let mut opts = rpm::FileOptions::new(entry.dest.clone())
                        .mode(rpm::FileMode::regular(mode));
                    opts = set_rpm_owner(opts, &entry.owner, &entry.group);

                    if *is_config {
                        opts = opts.is_config_noreplace();
                    }

                    builder = builder
                        .with_file(tmp.path(), opts)
                        .map_err(|e| Error::Rpm(e.to_string()))?;
                }
                EntryKind::Directory => {
                    let mode = mode_u16(entry.mode)?;
                    let tmp = temp_file(b"")?;

                    let mut opts =
                        rpm::FileOptions::new(entry.dest.clone()).mode(rpm::FileMode::dir(mode));
                    opts = set_rpm_owner(opts, &entry.owner, &entry.group);

                    builder = builder
                        .with_file(tmp.path(), opts)
                        .map_err(|e| Error::Rpm(e.to_string()))?;
                }
                EntryKind::Symlink { target } => {
                    let mode = mode_u16(entry.mode)?;
                    let tmp = temp_file(b"")?;

                    let mut opts = rpm::FileOptions::new(entry.dest.clone())
                        .mode(rpm::FileMode::symbolic_link(mode))
                        .symlink(target.clone());
                    opts = set_rpm_owner(opts, &entry.owner, &entry.group);

                    builder = builder
                        .with_file(tmp.path(), opts)
                        .map_err(|e| Error::Rpm(e.to_string()))?;
                }
            }
        }

        // Ghost files
        for ghost_path in &self.rpm.ghost_files {
            let tmp = temp_file(b"")?;
            let opts = rpm::FileOptions::new(ghost_path.clone())
                .mode(rpm::FileMode::regular(0o644))
                .is_ghost();
            builder = builder
                .with_file(tmp.path(), opts)
                .map_err(|e| Error::Rpm(e.to_string()))?;
        }

        // Shared scripts
        if let Some(ref mut s) = self.scripts.pre_install {
            builder = builder.pre_install_script(rpm::Scriptlet::new(s.read_string()?));
        }
        if let Some(ref mut s) = self.scripts.post_install {
            builder = builder.post_install_script(rpm::Scriptlet::new(s.read_string()?));
        }
        if let Some(ref mut s) = self.scripts.pre_remove {
            builder = builder.pre_uninstall_script(rpm::Scriptlet::new(s.read_string()?));
        }
        if let Some(ref mut s) = self.scripts.post_remove {
            builder = builder.post_uninstall_script(rpm::Scriptlet::new(s.read_string()?));
        }

        // RPM-specific scripts
        if let Some(ref mut s) = self.rpm.scripts.pre_trans {
            builder = builder.pre_trans_script(rpm::Scriptlet::new(s.read_string()?));
        }
        if let Some(ref mut s) = self.rpm.scripts.post_trans {
            builder = builder.post_trans_script(rpm::Scriptlet::new(s.read_string()?));
        }
        if let Some(ref mut s) = self.rpm.scripts.verify {
            builder = builder.verify_script(rpm::Scriptlet::new(s.read_string()?));
        }

        let pkg = builder.build().map_err(|e| Error::Rpm(e.to_string()))?;

        // rpm crate requires `impl Write` (Sized), but we have `dyn Write`.
        // Write to a buffer first, then copy.
        let mut buf = Vec::new();
        pkg.write(&mut buf).map_err(|e| Error::Rpm(e.to_string()))?;
        w.write_all(&buf)?;

        Ok(())
    }

    fn rpm_version_string(&self) -> String {
        self.version.clone()
    }
}

/// Parse a dependency string like "name >= 1.0" into an rpm::Dependency.
fn parse_rpm_dep(dep: &str) -> rpm::Dependency {
    // Split on first operator, trying longest operators first.
    let ops = [">=", "<=", ">>", "<<", ">", "<", "="];
    for op in ops {
        if let Some(idx) = dep.find(op) {
            let name = dep[..idx].trim();
            let version = dep[idx + op.len()..].trim();
            return match op {
                ">=" => rpm::Dependency::greater_eq(name, version),
                "<=" => rpm::Dependency::less_eq(name, version),
                ">>" | ">" => rpm::Dependency::greater(name, version),
                "<<" | "<" => rpm::Dependency::less(name, version),
                "=" => rpm::Dependency::eq(name, version),
                _ => unreachable!(),
            };
        }
    }
    rpm::Dependency::any(dep.trim())
}

fn mode_u16(mode: u32) -> Result<u16, Error> {
    mode.try_into()
        .map_err(|_| Error::Rpm(format!("mode {:#o} overflows u16", mode)))
}

fn set_rpm_owner(
    opts: rpm::FileOptionsBuilder,
    owner: &str,
    group: &str,
) -> rpm::FileOptionsBuilder {
    opts.user(owner).group(group)
}

/// Create a temporary file with the given contents.
/// Returns a NamedTempFile that auto-deletes on drop.
fn temp_file(data: &[u8]) -> Result<tempfile::NamedTempFile, Error> {
    use std::io::Write;
    let mut tmp = tempfile::NamedTempFile::new()?;
    tmp.write_all(data)?;
    tmp.flush()?;
    Ok(tmp)
}
