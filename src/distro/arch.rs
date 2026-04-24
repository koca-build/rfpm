//! Arch Linux package writer.
//!
//! The `.pkg.tar.zst` format is a zstd-compressed tar containing:
//! 1. Package files (regular files, dirs, symlinks)
//! 2. `.PKGINFO` — key-value metadata
//! 3. `.MTREE` — gzip-compressed manifest with md5+sha256 hashes
//! 4. `.INSTALL` (optional) — scripts wrapped in shell functions

use std::io::{self, Write};

use sha2::Digest;

use crate::{EntryKind, Error, Package};

impl Package {
    /// Write an Arch Linux `.pkg.tar.zst` package to the given writer.
    ///
    /// The format is a zstd-compressed tar containing:
    /// - Package files
    /// - `.PKGINFO` — key-value metadata
    /// - `.MTREE` — gzip-compressed manifest with md5+sha256 hashes
    /// - `.INSTALL` (optional) — scripts wrapped in shell functions
    pub fn write_arch(&mut self, w: &mut dyn Write) -> Result<(), Error> {
        if !is_valid_arch_name(&self.name) {
            return Err(Error::InvalidName(self.name.clone()));
        }

        let zw = zstd::Encoder::new(w, 19)?;
        let mut tw = tar::Builder::new(zw);

        // Write package files and collect mtree entries + total size.
        let (mut mtree_entries, total_size) = self.write_arch_files(&mut tw)?;

        // Write .PKGINFO and get its mtree entry.
        let pkginfo_entry = self.write_arch_pkginfo(&mut tw, total_size)?;

        // .PKGINFO must be first in .MTREE.
        mtree_entries.insert(0, pkginfo_entry);

        // Write .MTREE (gzip-compressed).
        write_arch_mtree(&mut tw, &mtree_entries)?;

        // Write .INSTALL if any scripts exist.
        self.write_arch_install(&mut tw)?;

        let zw = tw.into_inner()?;
        zw.finish()?;

        Ok(())
    }

    fn write_arch_files(
        &mut self,
        tw: &mut tar::Builder<impl Write>,
    ) -> Result<(Vec<MtreeEntry>, u64), Error> {
        let mut entries = Vec::new();
        let mut total_size: u64 = 0;

        for entry in &mut self.entries {
            let dest = entry.dest.strip_prefix('/').unwrap_or(&entry.dest);

            match &mut entry.kind {
                EntryKind::Directory => {
                    let mode = entry.mode.unwrap_or(0o755);
                    let mut header = tar::Header::new_gnu();
                    header.set_entry_type(tar::EntryType::Directory);
                    header.set_path(dest)?;
                    header.set_mode(mode);
                    header.set_size(0);
                    header.set_cksum();
                    tw.append(&header, io::empty())?;

                    entries.push(MtreeEntry {
                        path: dest.to_string(),
                        kind: MtreeKind::Dir,
                        mode: mode as i64,
                        time: 0,
                        size: 0,
                        md5: Vec::new(),
                        sha256: Vec::new(),
                        link_target: None,
                    });
                }
                EntryKind::Symlink { target } => {
                    let mut header = tar::Header::new_gnu();
                    header.set_entry_type(tar::EntryType::Symlink);
                    header.set_path(dest)?;
                    header.set_link_name(target.as_str())?;
                    header.set_mode(0o777);
                    header.set_size(0);
                    header.set_cksum();
                    tw.append(&header, io::empty())?;

                    entries.push(MtreeEntry {
                        path: dest.to_string(),
                        kind: MtreeKind::Link,
                        mode: 0o777,
                        time: 0,
                        size: 0,
                        md5: Vec::new(),
                        sha256: Vec::new(),
                        link_target: Some(target.clone()),
                    });
                }
                EntryKind::File { source, is_config } => {
                    let data = source.read_all()?;
                    let mode = entry.mode.unwrap_or(0o644);

                    let mut header = tar::Header::new_gnu();
                    header.set_entry_type(tar::EntryType::Regular);
                    header.set_path(dest)?;
                    header.set_mode(mode);
                    header.set_size(data.len() as u64);
                    header.set_cksum();
                    tw.append(&header, data.as_slice())?;

                    let md5_digest = md5::compute(&data);
                    let sha256_digest = sha2::Sha256::digest(&data);

                    entries.push(MtreeEntry {
                        path: dest.to_string(),
                        kind: if *is_config {
                            MtreeKind::ConfigFile
                        } else {
                            MtreeKind::File
                        },
                        mode: mode as i64,
                        time: 0,
                        size: data.len() as i64,
                        md5: md5_digest.to_vec(),
                        sha256: sha256_digest.to_vec(),
                        link_target: None,
                    });

                    total_size += data.len() as u64;
                }
            }
        }

        Ok((entries, total_size))
    }

    fn write_arch_pkginfo(
        &self,
        tw: &mut tar::Builder<impl Write>,
        total_size: u64,
    ) -> Result<MtreeEntry, Error> {
        let mut buf = String::new();
        buf.push_str("# Generated by rFPM\n");

        let pkgrel: u32 = self.release.parse().unwrap_or(1);
        let pkgver = if let Some(epoch) = self.epoch {
            format!("{}:{}-{}", epoch, self.version, pkgrel)
        } else {
            format!("{}-{}", self.version, pkgrel)
        };

        // Description cannot contain newlines.
        let pkgdesc = self.description.replace('\n', " ");

        write_kv(&mut buf, "pkgname", &self.name);
        write_kv(
            &mut buf,
            "pkgbase",
            self.arch_linux
                .pkgbase
                .as_deref()
                .unwrap_or(&self.name),
        );
        write_kv(&mut buf, "pkgver", &pkgver);
        write_kv(&mut buf, "pkgdesc", &pkgdesc);
        if let Some(ref hp) = self.homepage {
            write_kv(&mut buf, "url", hp);
        }
        write_kv(&mut buf, "builddate", "0");
        write_kv(
            &mut buf,
            "packager",
            self.arch_linux
                .packager
                .as_deref()
                .unwrap_or("Unknown Packager"),
        );
        write_kv(&mut buf, "size", &total_size.to_string());
        write_kv(&mut buf, "arch", self.arch.to_arch_linux());
        if let Some(ref license) = self.license {
            write_kv(&mut buf, "license", license);
        }

        for dep in &self.replaces {
            write_kv(&mut buf, "replaces", dep);
        }
        for dep in &self.conflicts {
            write_kv(&mut buf, "conflict", dep);
        }
        for dep in &self.provides {
            write_kv(&mut buf, "provides", dep);
        }
        for dep in &self.depends {
            write_kv(&mut buf, "depend", dep);
        }

        // Config files → backup entries
        for entry in &self.entries {
            if let EntryKind::File { is_config: true, .. } = &entry.kind {
                let path = entry.dest.strip_prefix('/').unwrap_or(&entry.dest);
                write_kv(&mut buf, "backup", path);
            }
        }

        let data = buf.as_bytes();
        let md5_digest = md5::compute(data);
        let sha256_digest = sha2::Sha256::digest(data);

        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_path(".PKGINFO")?;
        header.set_mode(0o644);
        header.set_size(data.len() as u64);
        header.set_cksum();
        tw.append(&header, data)?;

        Ok(MtreeEntry {
            path: ".PKGINFO".to_string(),
            kind: MtreeKind::File,
            mode: 0o644,
            time: 0,
            size: data.len() as i64,
            md5: md5_digest.to_vec(),
            sha256: sha256_digest.to_vec(),
            link_target: None,
        })
    }

    fn write_arch_install(&mut self, tw: &mut tar::Builder<impl Write>) -> Result<(), Error> {
        let mut install_buf = String::new();

        let script_entries: &mut [(
            &str,
            &mut Option<crate::Content>,
        )] = &mut [
            ("pre_install", &mut self.scripts.pre_install),
            ("post_install", &mut self.scripts.post_install),
            ("pre_remove", &mut self.scripts.pre_remove),
            ("post_remove", &mut self.scripts.post_remove),
            ("pre_upgrade", &mut self.arch_linux.scripts.pre_upgrade),
            ("post_upgrade", &mut self.arch_linux.scripts.post_upgrade),
        ];

        for (name, content) in script_entries.iter_mut() {
            if let Some(c) = content {
                let script_text = c.read_string()?;
                install_buf.push_str(&format!("function {}() {{\n", name));
                install_buf.push_str(&script_text);
                install_buf.push_str("\n}\n\n");
            }
        }

        if install_buf.is_empty() {
            return Ok(());
        }

        let data = install_buf.as_bytes();
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_path(".INSTALL")?;
        header.set_mode(0o644);
        header.set_size(data.len() as u64);
        header.set_cksum();
        tw.append(&header, data)?;

        Ok(())
    }
}

// --- Mtree ---

enum MtreeKind {
    File,
    ConfigFile,
    Dir,
    Link,
}

struct MtreeEntry {
    path: String,
    kind: MtreeKind,
    mode: i64,
    time: i64,
    size: i64,
    md5: Vec<u8>,
    sha256: Vec<u8>,
    link_target: Option<String>,
}

impl MtreeEntry {
    fn write_to(&self, w: &mut impl Write) -> io::Result<()> {
        match self.kind {
            MtreeKind::Dir => {
                writeln!(
                    w,
                    "./{} time={}.0 mode={:o} type=dir",
                    self.path, self.time, self.mode,
                )
            }
            MtreeKind::Link => {
                writeln!(
                    w,
                    "./{} time={}.0 mode={:o} type=link link={}",
                    self.path,
                    self.time,
                    self.mode,
                    self.link_target.as_deref().unwrap_or(""),
                )
            }
            MtreeKind::File | MtreeKind::ConfigFile => {
                writeln!(
                    w,
                    "./{} time={}.0 mode={:o} size={} type=file md5digest={} sha256digest={}",
                    self.path,
                    self.time,
                    self.mode,
                    self.size,
                    hex_encode(&self.md5),
                    hex_encode(&self.sha256),
                )
            }
        }
    }
}

fn write_arch_mtree(
    tw: &mut tar::Builder<impl Write>,
    entries: &[MtreeEntry],
) -> Result<(), Error> {
    let mut buf = Vec::new();
    let mut gz = flate2::write::GzEncoder::new(&mut buf, flate2::Compression::default());

    gz.write_all(b"#mtree\n")?;
    for entry in entries {
        entry.write_to(&mut gz)?;
    }
    gz.finish()?;

    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_path(".MTREE")?;
    header.set_mode(0o644);
    header.set_size(buf.len() as u64);
    header.set_cksum();
    tw.append(&header, buf.as_slice())?;

    Ok(())
}

// --- Helpers ---

fn write_kv(buf: &mut String, key: &str, value: &str) {
    if value.is_empty() {
        return;
    }
    buf.push_str(key);
    buf.push_str(" = ");
    buf.push_str(value);
    buf.push('\n');
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn is_valid_arch_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    if name.starts_with('-') || name.starts_with('.') {
        return false;
    }
    name.chars().all(|c| {
        c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '+' || c == '-'
    })
}
