//! Target architecture enum with per-format name mapping.

/// Target CPU architecture for the package.
///
/// Each variant automatically maps to the correct architecture string
/// for each package format (deb, rpm, arch linux).
///
/// ```
/// use rfpm::Arch;
///
/// assert_eq!(Arch::Amd64.to_deb(), "amd64");
/// assert_eq!(Arch::Amd64.to_rpm(), "x86_64");
/// assert_eq!(Arch::Amd64.to_arch_linux(), "x86_64");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Arch {
    /// x86_64 / amd64
    Amd64,
    /// 64-bit ARM
    Arm64,
    /// 32-bit x86
    I386,
    /// 32-bit ARM hard-float (ARMv7)
    Armhf,
    /// 32-bit ARM soft-float (ARMv5)
    Armel,
    /// Architecture-independent
    All,
}

impl Arch {
    /// Architecture string for Debian packages.
    pub fn to_deb(&self) -> &'static str {
        match self {
            Arch::Amd64 => "amd64",
            Arch::Arm64 => "arm64",
            Arch::I386 => "i386",
            Arch::Armhf => "armhf",
            Arch::Armel => "armel",
            Arch::All => "all",
        }
    }

    /// Architecture string for RPM packages.
    pub fn to_rpm(&self) -> &'static str {
        match self {
            Arch::Amd64 => "x86_64",
            Arch::Arm64 => "aarch64",
            Arch::I386 => "i386",
            Arch::Armhf => "armv7hl",
            Arch::Armel => "armv5tel",
            Arch::All => "noarch",
        }
    }

    /// Architecture string for Arch Linux packages.
    pub fn to_arch_linux(&self) -> &'static str {
        match self {
            Arch::Amd64 => "x86_64",
            Arch::Arm64 => "aarch64",
            Arch::I386 => "i686",
            Arch::Armhf => "armv7h",
            Arch::Armel => "arm",
            Arch::All => "any",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amd64_mappings() {
        assert_eq!(Arch::Amd64.to_deb(), "amd64");
        assert_eq!(Arch::Amd64.to_rpm(), "x86_64");
        assert_eq!(Arch::Amd64.to_arch_linux(), "x86_64");
    }

    #[test]
    fn arm64_mappings() {
        assert_eq!(Arch::Arm64.to_deb(), "arm64");
        assert_eq!(Arch::Arm64.to_rpm(), "aarch64");
        assert_eq!(Arch::Arm64.to_arch_linux(), "aarch64");
    }

    #[test]
    fn i386_mappings() {
        assert_eq!(Arch::I386.to_deb(), "i386");
        assert_eq!(Arch::I386.to_rpm(), "i386");
        assert_eq!(Arch::I386.to_arch_linux(), "i686");
    }

    #[test]
    fn armhf_mappings() {
        assert_eq!(Arch::Armhf.to_deb(), "armhf");
        assert_eq!(Arch::Armhf.to_rpm(), "armv7hl");
        assert_eq!(Arch::Armhf.to_arch_linux(), "armv7h");
    }

    #[test]
    fn armel_mappings() {
        assert_eq!(Arch::Armel.to_deb(), "armel");
        assert_eq!(Arch::Armel.to_rpm(), "armv5tel");
        assert_eq!(Arch::Armel.to_arch_linux(), "arm");
    }

    #[test]
    fn all_mappings() {
        assert_eq!(Arch::All.to_deb(), "all");
        assert_eq!(Arch::All.to_rpm(), "noarch");
        assert_eq!(Arch::All.to_arch_linux(), "any");
    }
}
