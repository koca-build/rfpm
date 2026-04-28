use std::io::Cursor;

use rfpm::{Arch, DebCompression, FileOptions, Package};

/// Create a test package with files, dirs, symlinks, config, and scripts.
fn sample_package() -> Package {
    let mut pkg = Package::new("testpkg", "1.0.0", Arch::Amd64, "A test package");
    pkg.release = "1".into();
    pkg.homepage = Some("https://example.com".into());
    pkg.license = Some("MIT".into());
    pkg.maintainer = Some("Test User <test@example.com>".into());
    pkg.depends.push("libc6".into());
    pkg.provides.push("testpkg-bin".into());

    pkg.add_file_with(
        "/usr/bin/testpkg",
        b"#!/bin/sh\necho hello\n".to_vec(),
        FileOptions {
            mode: 0o755,
            ..Default::default()
        },
    );
    pkg.add_file("/usr/share/testpkg/data.txt", b"some data\n".to_vec());
    pkg.add_config("/etc/testpkg/config.conf", b"key=value\n".to_vec());
    pkg.add_dir("/var/lib/testpkg");
    pkg.add_symlink("/usr/bin/testpkg-link", "/usr/bin/testpkg");

    pkg.scripts.post_install = Some("echo post-install ran".to_string().into());

    pkg
}

// --- Conventional filenames ---

#[test]
fn test_deb_filename() {
    let pkg = Package::new("myapp", "2.3.1", Arch::Amd64, "desc");
    assert_eq!(pkg.deb_filename(), "myapp_2.3.1-1_amd64.deb");
}

#[test]
fn test_rpm_filename() {
    let pkg = Package::new("myapp", "2.3.1", Arch::Amd64, "desc");
    assert_eq!(pkg.rpm_filename(), "myapp-2.3.1-1.x86_64.rpm");
}

#[test]
fn test_arch_filename() {
    let pkg = Package::new("myapp", "2.3.1", Arch::Amd64, "desc");
    assert_eq!(pkg.arch_filename(), "myapp-2.3.1-1-x86_64.pkg.tar.zst");
}

#[test]
fn test_filenames_arm64() {
    let pkg = Package::new("foo", "0.1.0", Arch::Arm64, "desc");
    assert_eq!(pkg.deb_filename(), "foo_0.1.0-1_arm64.deb");
    assert_eq!(pkg.rpm_filename(), "foo-0.1.0-1.aarch64.rpm");
    assert_eq!(pkg.arch_filename(), "foo-0.1.0-1-aarch64.pkg.tar.zst");
}

#[test]
fn test_filenames_all() {
    let pkg = Package::new("data", "1.0.0", Arch::All, "desc");
    assert_eq!(pkg.deb_filename(), "data_1.0.0-1_all.deb");
    assert_eq!(pkg.rpm_filename(), "data-1.0.0-1.noarch.rpm");
    assert_eq!(pkg.arch_filename(), "data-1.0.0-1-any.pkg.tar.zst");
}

// --- DEB roundtrip ---

#[test]
fn test_deb_roundtrip() {
    let mut pkg = sample_package();

    let mut deb_bytes = Vec::new();
    pkg.write_deb(&mut deb_bytes).expect("write_deb failed");

    // Should be a valid AR archive.
    assert!(
        deb_bytes.len() > 100,
        "deb too small: {} bytes",
        deb_bytes.len()
    );

    // AR magic: "!<arch>\n"
    assert_eq!(&deb_bytes[..8], b"!<arch>\n", "missing AR magic");

    // Parse the AR archive and verify members.
    let mut archive = ar::Archive::new(Cursor::new(&deb_bytes));
    let mut member_names = Vec::new();

    while let Some(entry) = archive.next_entry() {
        let entry = entry.expect("bad ar entry");
        let name = String::from_utf8_lossy(entry.header().identifier()).to_string();
        member_names.push(name);
    }

    assert!(
        member_names.contains(&"debian-binary".to_string()),
        "missing debian-binary"
    );
    assert!(
        member_names.contains(&"control.tar.gz".to_string()),
        "missing control.tar.gz"
    );
    assert!(
        member_names.iter().any(|n| n.starts_with("data.tar")),
        "missing data.tar.*"
    );

    // Verify debian-binary content.
    let mut archive = ar::Archive::new(Cursor::new(&deb_bytes));
    while let Some(entry) = archive.next_entry() {
        let mut entry = entry.unwrap();
        let name = String::from_utf8_lossy(entry.header().identifier()).to_string();
        if name == "debian-binary" {
            let mut content = String::new();
            std::io::Read::read_to_string(&mut entry, &mut content).unwrap();
            assert_eq!(content, "2.0\n");
        }
    }
}

#[test]
fn test_deb_control_has_metadata() {
    let mut pkg = sample_package();
    let mut deb_bytes = Vec::new();
    pkg.write_deb(&mut deb_bytes).unwrap();

    // Extract control.tar.gz from the AR archive.
    let mut archive = ar::Archive::new(Cursor::new(&deb_bytes));
    let mut control_gz = Vec::new();
    while let Some(entry) = archive.next_entry() {
        let mut entry = entry.unwrap();
        let name = String::from_utf8_lossy(entry.header().identifier()).to_string();
        if name == "control.tar.gz" {
            std::io::Read::read_to_end(&mut entry, &mut control_gz).unwrap();
        }
    }
    assert!(!control_gz.is_empty(), "control.tar.gz not found");

    // Decompress and find the control file.
    let gz = flate2::read::GzDecoder::new(Cursor::new(&control_gz));
    let mut tar = tar::Archive::new(gz);
    let mut control_text = String::new();
    let mut has_md5sums = false;
    let mut has_conffiles = false;
    let mut has_postinst = false;

    let mut control_tar_paths = Vec::new();
    for entry in tar.entries().unwrap() {
        let mut entry = entry.unwrap();
        let path = entry.path().unwrap().to_string_lossy().to_string();
        control_tar_paths.push(path.clone());
        if path == "control" || path == "./control" {
            std::io::Read::read_to_string(&mut entry, &mut control_text).unwrap();
        } else if path.contains("md5sums") {
            has_md5sums = true;
        } else if path.contains("conffiles") {
            has_conffiles = true;
        } else if path.contains("postinst") {
            has_postinst = true;
        }
    }

    assert!(
        control_text.contains("Package: testpkg"),
        "missing Package field. control_tar_paths={control_tar_paths:?}, control_gz_len={}, control_text={control_text:?}",
        control_gz.len(),
    );
    assert!(
        control_text.contains("Version:"),
        "missing Version field in:\n{control_text}"
    );
    assert!(
        control_text.contains("Architecture: amd64"),
        "missing Architecture field in:\n{control_text}"
    );
    assert!(
        control_text.contains("Depends: libc6"),
        "missing Depends field in:\n{control_text}"
    );
    assert!(
        control_text.contains("Provides: testpkg-bin"),
        "missing Provides field in:\n{control_text}"
    );
    assert!(
        control_text.contains("Homepage: https://example.com"),
        "missing Homepage in:\n{control_text}"
    );
    assert!(
        control_text.contains("Description: A test package"),
        "missing Description in:\n{control_text}"
    );
    assert!(has_md5sums, "missing md5sums file");
    assert!(has_conffiles, "missing conffiles file");
    assert!(has_postinst, "missing postinst script");
}

#[test]
fn test_deb_data_has_files() {
    let mut pkg = sample_package();
    let mut deb_bytes = Vec::new();
    pkg.write_deb(&mut deb_bytes).unwrap();

    // Extract data.tar.xz from the AR archive.
    let mut archive = ar::Archive::new(Cursor::new(&deb_bytes));
    let mut data_xz = Vec::new();
    while let Some(entry) = archive.next_entry() {
        let mut entry = entry.unwrap();
        let name = String::from_utf8_lossy(entry.header().identifier()).to_string();
        if name.starts_with("data.tar") {
            std::io::Read::read_to_end(&mut entry, &mut data_xz).unwrap();
        }
    }
    assert!(!data_xz.is_empty(), "data.tar.* not found");

    let xz = liblzma::read::XzDecoder::new(Cursor::new(&data_xz));
    let mut tar = tar::Archive::new(xz);
    let mut paths: Vec<String> = Vec::new();

    for entry in tar.entries().unwrap() {
        let entry = entry.unwrap();
        paths.push(entry.path().unwrap().to_string_lossy().to_string());
    }

    assert!(
        paths.iter().any(|p| p.contains("usr/bin/testpkg")),
        "missing binary"
    );
    assert!(
        paths
            .iter()
            .any(|p| p.contains("usr/share/testpkg/data.txt")),
        "missing data file"
    );
    assert!(
        paths.iter().any(|p| p.contains("etc/testpkg/config.conf")),
        "missing config"
    );
    assert!(
        paths.iter().any(|p| p.contains("var/lib/testpkg")),
        "missing directory"
    );
    assert!(
        paths.iter().any(|p| p.contains("usr/bin/testpkg-link")),
        "missing symlink"
    );
}

#[test]
fn test_deb_compression_xz() {
    let mut pkg = Package::new("xztest", "1.0.0", Arch::Amd64, "xz test");
    pkg.deb.compression = DebCompression::Xz;
    pkg.add_file("/usr/bin/hello", b"hello".to_vec());

    let mut buf = Vec::new();
    pkg.write_deb(&mut buf).expect("write_deb with xz failed");

    // Verify data.tar.xz member exists.
    let mut archive = ar::Archive::new(Cursor::new(&buf));
    let mut found_xz = false;
    while let Some(entry) = archive.next_entry() {
        let entry = entry.unwrap();
        let name = String::from_utf8_lossy(entry.header().identifier()).to_string();
        if name == "data.tar.xz" {
            found_xz = true;
        }
    }
    assert!(found_xz, "data.tar.xz not found");
}

#[test]
fn test_deb_compression_zstd() {
    let mut pkg = Package::new("zstdtest", "1.0.0", Arch::Amd64, "zstd test");
    pkg.deb.compression = DebCompression::Zstd;
    pkg.add_file("/usr/bin/hello", b"hello".to_vec());

    let mut buf = Vec::new();
    pkg.write_deb(&mut buf).expect("write_deb with zstd failed");

    let mut archive = ar::Archive::new(Cursor::new(&buf));
    let mut found_zst = false;
    while let Some(entry) = archive.next_entry() {
        let entry = entry.unwrap();
        let name = String::from_utf8_lossy(entry.header().identifier()).to_string();
        if name == "data.tar.zst" {
            found_zst = true;
        }
    }
    assert!(found_zst, "data.tar.zst not found");
}

// --- RPM roundtrip ---

#[test]
fn test_rpm_roundtrip() {
    let mut pkg = sample_package();

    let mut rpm_bytes = Vec::new();
    pkg.write_rpm(&mut rpm_bytes).expect("write_rpm failed");

    assert!(
        rpm_bytes.len() > 100,
        "rpm too small: {} bytes",
        rpm_bytes.len()
    );

    // RPM magic: 0xED 0xAB 0xEE 0xDB
    assert_eq!(
        &rpm_bytes[..4],
        &[0xED, 0xAB, 0xEE, 0xDB],
        "missing RPM magic"
    );
}

// --- Arch roundtrip ---

#[test]
fn test_arch_roundtrip() {
    let mut pkg = sample_package();

    let mut arch_bytes = Vec::new();
    pkg.write_arch(&mut arch_bytes).expect("write_arch failed");

    assert!(
        arch_bytes.len() > 100,
        "arch pkg too small: {} bytes",
        arch_bytes.len()
    );

    // Decompress zstd and parse tar.
    let zr = zstd::Decoder::new(Cursor::new(&arch_bytes)).unwrap();
    let mut tar = tar::Archive::new(zr);
    let mut paths: Vec<String> = Vec::new();
    let mut has_pkginfo = false;
    let mut has_mtree = false;
    let mut has_install = false;
    let mut pkginfo_text = String::new();

    for entry in tar.entries().unwrap() {
        let mut entry = entry.unwrap();
        let path = entry.path().unwrap().to_string_lossy().to_string();

        if path == ".PKGINFO" {
            has_pkginfo = true;
            std::io::Read::read_to_string(&mut entry, &mut pkginfo_text).unwrap();
        } else if path == ".MTREE" {
            has_mtree = true;
        } else if path == ".INSTALL" {
            has_install = true;
        } else {
            paths.push(path);
        }
    }

    assert!(has_pkginfo, "missing .PKGINFO");
    assert!(has_mtree, "missing .MTREE");
    assert!(
        has_install,
        "should have .INSTALL because postinst script was set"
    );

    // Check .PKGINFO content.
    assert!(
        pkginfo_text.contains("pkgname = testpkg"),
        "missing pkgname"
    );
    assert!(pkginfo_text.contains("pkgver = 1.0.0-1"), "missing pkgver");
    assert!(pkginfo_text.contains("arch = x86_64"), "missing arch");
    assert!(pkginfo_text.contains("depend = libc6"), "missing depend");
    assert!(
        pkginfo_text.contains("provides = testpkg-bin"),
        "missing provides"
    );
    assert!(pkginfo_text.contains("license = MIT"), "missing license");

    // Config files should be in backup.
    assert!(
        pkginfo_text.contains("backup = etc/testpkg/config.conf"),
        "missing backup entry"
    );

    // Check files are present.
    assert!(
        paths.iter().any(|p| p.contains("usr/bin/testpkg")),
        "missing binary"
    );
    assert!(
        paths
            .iter()
            .any(|p| p.contains("usr/share/testpkg/data.txt")),
        "missing data"
    );
    assert!(
        paths.iter().any(|p| p.contains("etc/testpkg/config.conf")),
        "missing config"
    );
}

#[test]
fn test_arch_invalid_name() {
    let mut pkg = Package::new("-invalid", "1.0.0", Arch::Amd64, "desc");
    let mut buf = Vec::new();
    let result = pkg.write_arch(&mut buf);
    assert!(result.is_err(), "should reject names starting with -");
}

#[test]
fn test_arch_mtree_is_gzipped() {
    let mut pkg = Package::new("mtreetest", "1.0.0", Arch::Amd64, "test");
    pkg.add_file("/usr/bin/hello", b"hello world".to_vec());

    let mut arch_bytes = Vec::new();
    pkg.write_arch(&mut arch_bytes).unwrap();

    let zr = zstd::Decoder::new(Cursor::new(&arch_bytes)).unwrap();
    let mut tar = tar::Archive::new(zr);

    for entry in tar.entries().unwrap() {
        let mut entry = entry.unwrap();
        let path = entry.path().unwrap().to_string_lossy().to_string();
        if path == ".MTREE" {
            let mut mtree_gz = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut mtree_gz).unwrap();

            // Gzip magic: 0x1F 0x8B
            assert_eq!(
                &mtree_gz[..2],
                &[0x1F, 0x8B],
                ".MTREE should be gzip-compressed"
            );

            // Decompress and check content.
            let mut gz = flate2::read::GzDecoder::new(Cursor::new(&mtree_gz));
            let mut mtree_text = String::new();
            std::io::Read::read_to_string(&mut gz, &mut mtree_text).unwrap();

            assert!(
                mtree_text.starts_with("#mtree\n"),
                "should start with #mtree"
            );
            assert!(
                mtree_text.contains(".PKGINFO"),
                ".PKGINFO should be first in mtree"
            );
            assert!(
                mtree_text.contains("sha256digest="),
                "should have sha256 hashes"
            );
            assert!(mtree_text.contains("md5digest="), "should have md5 hashes");
        }
    }
}

// --- Multi-format ---

#[test]
fn test_multi_format_same_package() {
    let mut pkg = sample_package();

    let mut deb = Vec::new();
    pkg.write_deb(&mut deb).expect("deb failed");

    let mut rpm = Vec::new();
    pkg.write_rpm(&mut rpm).expect("rpm failed after deb");

    let mut arch = Vec::new();
    pkg.write_arch(&mut arch).expect("arch failed after rpm");

    // All should produce non-empty output.
    assert!(!deb.is_empty());
    assert!(!rpm.is_empty());
    assert!(!arch.is_empty());

    // Verify formats via magic bytes.
    assert_eq!(&deb[..8], b"!<arch>\n", "deb magic");
    assert_eq!(&rpm[..4], &[0xED, 0xAB, 0xEE, 0xDB], "rpm magic");
    // zstd magic: 0x28 0xB5 0x2F 0xFD
    assert_eq!(&arch[..4], &[0x28, 0xB5, 0x2F, 0xFD], "arch zstd magic");
}

// --- Mode overflow ---

#[test]
fn test_rpm_rejects_mode_overflow() {
    let mut pkg = Package::new("testpkg", "1.0.0", Arch::Amd64, "test");
    pkg.add_file_with(
        "/usr/bin/x",
        b"data".to_vec(),
        FileOptions {
            mode: 0o200000, // overflows u16
            ..Default::default()
        },
    );

    let mut buf = Vec::new();
    let err = pkg.write_rpm(&mut buf);
    assert!(err.is_err(), "should reject mode that overflows u16");
    let msg = err.unwrap_err().to_string();
    assert!(
        msg.contains("overflows u16"),
        "error should mention overflow, got: {msg}"
    );
}
