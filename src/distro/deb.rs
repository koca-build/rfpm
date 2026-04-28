//! DEB package writer.
//!
//! A `.deb` is an AR archive containing:
//! 1. `debian-binary` — format version (`"2.0\n"`)
//! 2. `control.tar.gz` — metadata, md5sums, conffiles, triggers, scripts
//! 3. `data.tar.{gz,xz,zst}` — the actual package files

use std::fmt::Write as _;
use std::io::{self, Write};

use crate::{DebCompression, DebTriggers, EntryKind, Error, Package};

impl Package {
    /// Write a `.deb` package to the given writer.
    ///
    /// The deb format is an AR archive containing:
    /// - `debian-binary` — format version (`"2.0\n"`)
    /// - `control.tar.gz` — package metadata, md5sums, scripts
    /// - `data.tar.{gz,xz,zst}` — the actual package files
    pub fn write_deb(&mut self, w: &mut dyn Write) -> Result<(), Error> {
        // Build data tarball first — we need md5sums and installed size for the control file.
        let (data_tarball, data_name, md5sums, inst_size) = self.build_deb_data()?;

        // Build control tarball.
        let control_tarball = self.build_deb_control(&md5sums, inst_size)?;

        let debian_binary = b"2.0\n";

        // Write AR archive.
        let mut ar = ar::Builder::new(w);

        write_ar_entry(&mut ar, "debian-binary", debian_binary)?;
        write_ar_entry(&mut ar, "control.tar.gz", &control_tarball)?;
        write_ar_entry(&mut ar, &data_name, &data_tarball)?;

        Ok(())
    }

    /// Build `data.tar.*` — returns (tarball bytes, filename, md5sums text, installed size).
    fn build_deb_data(&mut self) -> Result<(Vec<u8>, String, String, u64), Error> {
        // Build uncompressed tar first.
        let mut tar_buf = Vec::new();
        let mut md5_lines = String::new();
        let mut inst_size: u64 = 0;

        {
            let mut tar = tar::Builder::new(&mut tar_buf);

            for entry in &mut self.entries {
                let dest = normalize_deb_path(&entry.dest);
                match &mut entry.kind {
                    EntryKind::Directory => {
                        let mut header = tar::Header::new_gnu();
                        header.set_entry_type(tar::EntryType::Directory);
                        header.set_path(&dest)?;
                        header.set_mode(entry.mode);
                        header.set_size(0);
                        set_owner(&mut header, &entry.owner, &entry.group);
                        header.set_cksum();
                        tar.append(&header, io::empty())?;
                    }
                    EntryKind::Symlink { target } => {
                        let mut header = tar::Header::new_gnu();
                        header.set_entry_type(tar::EntryType::Symlink);
                        header.set_path(&dest)?;
                        header.set_link_name(target.as_str())?;
                        header.set_mode(entry.mode);
                        header.set_size(0);
                        set_owner(&mut header, &entry.owner, &entry.group);
                        header.set_cksum();
                        tar.append(&header, io::empty())?;
                    }
                    EntryKind::File { source, .. } => {
                        let data = source.read_all()?;
                        let digest = md5::compute(&data);

                        let mut header = tar::Header::new_gnu();
                        header.set_entry_type(tar::EntryType::Regular);
                        header.set_path(&dest)?;
                        header.set_mode(entry.mode);
                        header.set_size(data.len() as u64);
                        set_owner(&mut header, &entry.owner, &entry.group);
                        header.set_cksum();
                        tar.append(&header, data.as_slice())?;

                        // md5sums: two spaces between hash and relative path
                        let rel_path = dest.strip_prefix("./").unwrap_or(&dest);
                        writeln!(&mut md5_lines, "{:x}  {}", digest, rel_path).unwrap();

                        inst_size += data.len() as u64;
                    }
                }
            }

            tar.finish()?;
        }

        // Compress the tar.
        let (compressed, data_name) = compress_deb_data(&tar_buf, self.deb.compression)?;

        Ok((compressed, data_name, md5_lines, inst_size))
    }

    /// Build `control.tar.gz`.
    fn build_deb_control(&mut self, md5sums: &str, inst_size: u64) -> Result<Vec<u8>, Error> {
        let mut buf = Vec::new();
        let gz = flate2::write::GzEncoder::new(&mut buf, flate2::Compression::default());
        let mut tar = tar::Builder::new(gz);

        // ./control
        let control = self.render_deb_control(inst_size);
        write_tar_file(&mut tar, "./control", control.as_bytes(), 0o644)?;

        // ./md5sums
        write_tar_file(&mut tar, "./md5sums", md5sums.as_bytes(), 0o644)?;

        // ./conffiles
        let conffiles = self.render_deb_conffiles();
        if !conffiles.is_empty() {
            write_tar_file(&mut tar, "./conffiles", conffiles.as_bytes(), 0o644)?;
        }

        // ./triggers
        let triggers = render_deb_triggers(&self.deb.triggers);
        if !triggers.is_empty() {
            write_tar_file(&mut tar, "./triggers", triggers.as_bytes(), 0o644)?;
        }

        // Shared scripts
        if let Some(ref mut s) = self.scripts.pre_install {
            write_tar_file(&mut tar, "./preinst", &s.read_all()?, 0o755)?;
        }
        if let Some(ref mut s) = self.scripts.post_install {
            write_tar_file(&mut tar, "./postinst", &s.read_all()?, 0o755)?;
        }
        if let Some(ref mut s) = self.scripts.pre_remove {
            write_tar_file(&mut tar, "./prerm", &s.read_all()?, 0o755)?;
        }
        if let Some(ref mut s) = self.scripts.post_remove {
            write_tar_file(&mut tar, "./postrm", &s.read_all()?, 0o755)?;
        }

        // Deb-specific scripts
        if let Some(ref mut s) = self.deb.scripts.rules {
            write_tar_file(&mut tar, "./rules", &s.read_all()?, 0o755)?;
        }
        if let Some(ref mut s) = self.deb.scripts.templates {
            write_tar_file(&mut tar, "./templates", &s.read_all()?, 0o644)?;
        }
        if let Some(ref mut s) = self.deb.scripts.config {
            write_tar_file(&mut tar, "./config", &s.read_all()?, 0o755)?;
        }

        let gz = tar.into_inner()?;
        gz.finish()?;

        Ok(buf)
    }

    /// Render the `control` file text.
    fn render_deb_control(&self, inst_size: u64) -> String {
        let mut c = String::new();

        // Mandatory fields
        push_field(&mut c, "Package", &self.name);
        push_field(&mut c, "Version", &self.deb_version_string());
        if let Some(ref section) = self.deb.section {
            push_field(&mut c, "Section", section);
        }
        push_field(&mut c, "Priority", &self.deb.priority);
        push_field(&mut c, "Architecture", self.arch.to_deb());
        if let Some(ref m) = self.maintainer {
            push_field(&mut c, "Maintainer", m);
        }
        c.push_str(&format!("Installed-Size: {}\n", inst_size / 1024));

        // Dependency fields
        push_list(&mut c, "Replaces", &self.replaces);
        push_list(&mut c, "Provides", &self.provides);
        push_list(&mut c, "Pre-Depends", &self.deb.predepends);
        push_list(&mut c, "Depends", &self.depends);
        push_list(&mut c, "Recommends", &self.recommends);
        push_list(&mut c, "Suggests", &self.suggests);
        push_list(&mut c, "Conflicts", &self.conflicts);
        push_list(&mut c, "Breaks", &self.deb.breaks);

        if let Some(ref hp) = self.homepage {
            push_field(&mut c, "Homepage", hp);
        }

        // Description with continuation lines
        push_field(
            &mut c,
            "Description",
            &format_deb_description(&self.description),
        );

        // Custom fields
        for (key, value) in &self.deb.fields {
            if !value.is_empty() {
                push_field(&mut c, key, value);
            }
        }

        c
    }

    /// Build the deb version string: `[epoch:]version[-release]`.
    fn deb_version_string(&self) -> String {
        let mut v = String::new();
        if let Some(epoch) = self.epoch {
            v.push_str(&format!("{epoch}:"));
        }
        v.push_str(&self.version);
        v.push('-');
        v.push_str(&self.release);
        v
    }

    /// Render the conffiles content — one absolute path per line for config entries.
    fn render_deb_conffiles(&self) -> String {
        let mut out = String::new();
        for entry in &self.entries {
            if let EntryKind::File {
                is_config: true, ..
            } = &entry.kind
            {
                let path = if entry.dest.starts_with('/') {
                    entry.dest.clone()
                } else {
                    format!("/{}", entry.dest)
                };
                out.push_str(&path);
                out.push('\n');
            }
        }
        out
    }
}

// --- Helpers ---

fn compress_deb_data(
    tar_bytes: &[u8],
    compression: DebCompression,
) -> Result<(Vec<u8>, String), Error> {
    let mut out = Vec::new();
    let name = match compression {
        DebCompression::Gzip => {
            let mut enc = flate2::write::GzEncoder::new(&mut out, flate2::Compression::new(9));
            enc.write_all(tar_bytes)?;
            enc.finish()?;
            "data.tar.gz".to_string()
        }
        DebCompression::Xz => {
            let mut enc = liblzma::write::XzEncoder::new(&mut out, 6);
            enc.write_all(tar_bytes)?;
            enc.finish()?;
            "data.tar.xz".to_string()
        }
        DebCompression::Zstd => {
            let mut enc = zstd::Encoder::new(&mut out, 19)?;
            enc.write_all(tar_bytes)?;
            enc.finish()?;
            "data.tar.zst".to_string()
        }
        DebCompression::None => {
            out.extend_from_slice(tar_bytes);
            "data.tar".to_string()
        }
    };
    Ok((out, name))
}

fn normalize_deb_path(dest: &str) -> String {
    let clean = dest.strip_prefix('/').unwrap_or(dest);
    format!("./{clean}")
}

fn set_owner(header: &mut tar::Header, owner: &str, group: &str) {
    header.set_username(owner).ok();
    header.set_groupname(group).ok();
    header.set_uid(0);
    header.set_gid(0);
}

fn write_tar_file(
    tar: &mut tar::Builder<impl Write>,
    path: &str,
    data: &[u8],
    mode: u32,
) -> Result<(), Error> {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_path(path)?;
    header.set_mode(mode);
    header.set_size(data.len() as u64);
    header.set_uid(0);
    header.set_gid(0);
    header.set_username("root").ok();
    header.set_groupname("root").ok();
    header.set_cksum();
    tar.append(&header, data)?;
    Ok(())
}

fn write_ar_entry(
    ar: &mut ar::Builder<&mut dyn Write>,
    name: &str,
    data: &[u8],
) -> Result<(), Error> {
    let mut header = ar::Header::new(name.as_bytes().to_vec(), data.len() as u64);
    header.set_mode(0o644);
    header.set_uid(0);
    header.set_gid(0);
    ar.append(&header, data)?;
    Ok(())
}

fn render_deb_triggers(triggers: &DebTriggers) -> String {
    let mut out = String::new();
    let entries: &[(&str, &[String])] = &[
        ("interest", &triggers.interest),
        ("interest-await", &triggers.interest_await),
        ("interest-noawait", &triggers.interest_noawait),
        ("activate", &triggers.activate),
        ("activate-await", &triggers.activate_await),
        ("activate-noawait", &triggers.activate_noawait),
    ];
    for (directive, names) in entries {
        for name in *names {
            out.push_str(directive);
            out.push(' ');
            out.push_str(name);
            out.push('\n');
        }
    }
    out
}

fn push_field(out: &mut String, key: &str, value: &str) {
    out.push_str(key);
    out.push_str(": ");
    out.push_str(value);
    out.push('\n');
}

fn push_list(out: &mut String, key: &str, items: &[String]) {
    if !items.is_empty() {
        push_field(out, key, &items.join(", "));
    }
}

/// Format a description for the deb control file.
/// First line is the synopsis, additional lines are prefixed with a space.
/// Empty continuation lines become ` .`.
fn format_deb_description(desc: &str) -> String {
    let mut lines = desc.lines();
    let mut out = String::new();
    if let Some(first) = lines.next() {
        out.push_str(first.trim());
    }
    for line in lines {
        out.push_str("\n ");
        let trimmed = line.trim();
        if trimmed.is_empty() {
            out.push('.');
        } else {
            out.push_str(trimmed);
        }
    }
    out
}
