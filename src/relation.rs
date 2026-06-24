//! Package relationship types.
//!
//! [`Relation`] models one entry in a versioned relationship field (`depends`,
//! `conflicts`, `replaces`, and deb's pre-depends/breaks). [`VirtualPackage`]
//! models one entry in `provides`, which may only be pinned to an *exact*
//! version (deb and pacman both reject `<`/`>` operators for provides).
//!
//! Versions are opaque strings: deb, rpm, pacman and apk each use their own
//! version grammar and comparison algorithm (none of them semver), so the
//! version is stored verbatim and rendered into each format as-is.

use std::fmt;

/// A version-comparison operator in a [`Constraint`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// `<` (deb `<<`).
    Less,
    /// `<=`.
    LessEqual,
    /// `=`.
    Equal,
    /// `>=`.
    GreaterEqual,
    /// `>` (deb `>>`).
    Greater,
}

impl Op {
    /// The Debian control-file spelling. Strict operators are doubled
    /// (`<<`/`>>`) per Debian policy.
    pub fn as_deb(self) -> &'static str {
        match self {
            Op::Less => "<<",
            Op::LessEqual => "<=",
            Op::Equal => "=",
            Op::GreaterEqual => ">=",
            Op::Greater => ">>",
        }
    }

    /// The pacman/`.PKGINFO` spelling (also used for the plain text form).
    pub fn as_pacman(self) -> &'static str {
        match self {
            Op::Less => "<",
            Op::LessEqual => "<=",
            Op::Equal => "=",
            Op::GreaterEqual => ">=",
            Op::Greater => ">",
        }
    }
}

/// A version constraint: an operator paired with a version string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Constraint {
    pub op: Op,
    pub version: String,
}

/// One entry in a versioned relationship field — a package name with an
/// optional version [`Constraint`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relation {
    pub name: String,
    pub constraint: Option<Constraint>,
}

/// One entry in the `provides` field — a virtual package name, optionally
/// pinned to an exact (`=`) version. Provides cannot use range operators.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualPackage {
    pub name: String,
    pub version: Option<String>,
}

/// Error returned when a relationship string cannot be parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationError {
    /// The name part was empty (e.g. `">=1.0"`).
    EmptyName,
    /// An operator was present but no version followed (e.g. `"foo>="`).
    EmptyVersion,
    /// A range operator was used where only `=` is allowed (provides).
    NonEqualProvides,
}

impl fmt::Display for RelationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            RelationError::EmptyName => "relationship has no package name",
            RelationError::EmptyVersion => "relationship has no version after operator",
            RelationError::NonEqualProvides => "provides may only use '=' (no '<' or '>')",
        };
        f.write_str(msg)
    }
}

impl std::error::Error for RelationError {}

// Operators tried longest-first so ">=" isn't split as ">".
const OPS: &[(&str, Op)] = &[
    (">=", Op::GreaterEqual),
    ("<=", Op::LessEqual),
    (">", Op::Greater),
    ("<", Op::Less),
    ("=", Op::Equal),
];

/// Split `s` into `(name, op, version)` on the first matching operator.
fn split_op(s: &str) -> Option<(&str, Op, &str)> {
    for (token, op) in OPS {
        if let Some((name, version)) = s.split_once(token) {
            return Some((name, *op, version));
        }
    }
    None
}

impl Relation {
    /// Parse operator syntax like `"openssl>=3.0"` or a bare `"curl"`.
    pub fn parse(s: &str) -> Result<Self, RelationError> {
        match split_op(s) {
            Some((name, op, version)) => {
                let (name, version) = (name.trim(), version.trim());
                if name.is_empty() {
                    return Err(RelationError::EmptyName);
                }
                if version.is_empty() {
                    return Err(RelationError::EmptyVersion);
                }
                Ok(Relation {
                    name: name.to_string(),
                    constraint: Some(Constraint {
                        op,
                        version: version.to_string(),
                    }),
                })
            }
            None => {
                let name = s.trim();
                if name.is_empty() {
                    return Err(RelationError::EmptyName);
                }
                Ok(Relation {
                    name: name.to_string(),
                    constraint: None,
                })
            }
        }
    }

    /// Render for a Debian control file, e.g. `openssl (>= 3.0)`.
    pub fn to_deb(&self) -> String {
        match &self.constraint {
            Some(c) => format!("{} ({} {})", self.name, c.op.as_deb(), c.version),
            None => self.name.clone(),
        }
    }

    /// Render for a pacman `.PKGINFO`, e.g. `openssl>=3.0`.
    pub fn to_pacman(&self) -> String {
        match &self.constraint {
            Some(c) => format!("{}{}{}", self.name, c.op.as_pacman(), c.version),
            None => self.name.clone(),
        }
    }
}

impl VirtualPackage {
    /// Parse `"name"` or an exact `"name=1.0"`. Range operators are rejected.
    pub fn parse(s: &str) -> Result<Self, RelationError> {
        match split_op(s) {
            Some((_, op, _)) if op != Op::Equal => Err(RelationError::NonEqualProvides),
            Some((name, _, version)) => {
                let (name, version) = (name.trim(), version.trim());
                if name.is_empty() {
                    return Err(RelationError::EmptyName);
                }
                if version.is_empty() {
                    return Err(RelationError::EmptyVersion);
                }
                Ok(VirtualPackage {
                    name: name.to_string(),
                    version: Some(version.to_string()),
                })
            }
            None => {
                let name = s.trim();
                if name.is_empty() {
                    return Err(RelationError::EmptyName);
                }
                Ok(VirtualPackage {
                    name: name.to_string(),
                    version: None,
                })
            }
        }
    }

    /// Render for a Debian control file, e.g. `bun (= 1.3.11)`.
    pub fn to_deb(&self) -> String {
        match &self.version {
            Some(v) => format!("{} (= {})", self.name, v),
            None => self.name.clone(),
        }
    }

    /// Render for a pacman `.PKGINFO`, e.g. `bun=1.3.11`.
    pub fn to_pacman(&self) -> String {
        match &self.version {
            Some(v) => format!("{}={}", self.name, v),
            None => self.name.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bare() {
        let r = Relation::parse("curl").unwrap();
        assert_eq!(r.name, "curl");
        assert!(r.constraint.is_none());
    }

    #[test]
    fn parse_ge_not_gt() {
        let r = Relation::parse("openssl>=3.0").unwrap();
        assert_eq!(r.name, "openssl");
        let c = r.constraint.unwrap();
        assert_eq!(c.op, Op::GreaterEqual);
        assert_eq!(c.version, "3.0");
    }

    #[test]
    fn parse_errors() {
        assert_eq!(Relation::parse(">=1.0"), Err(RelationError::EmptyName));
        assert_eq!(Relation::parse("foo>="), Err(RelationError::EmptyVersion));
    }

    #[test]
    fn relation_render() {
        let r = Relation::parse("openssl>=3.0").unwrap();
        assert_eq!(r.to_deb(), "openssl (>= 3.0)");
        assert_eq!(r.to_pacman(), "openssl>=3.0");

        let lt = Relation::parse("python<4").unwrap();
        assert_eq!(lt.to_deb(), "python (<< 4)");
        assert_eq!(lt.to_pacman(), "python<4");

        let bare = Relation::parse("curl").unwrap();
        assert_eq!(bare.to_deb(), "curl");
        assert_eq!(bare.to_pacman(), "curl");
    }

    #[test]
    fn provides_equal_only() {
        let v = VirtualPackage::parse("bun=1.3.11").unwrap();
        assert_eq!(v.name, "bun");
        assert_eq!(v.version.as_deref(), Some("1.3.11"));
        assert_eq!(v.to_deb(), "bun (= 1.3.11)");
        assert_eq!(v.to_pacman(), "bun=1.3.11");

        let bare = VirtualPackage::parse("bun").unwrap();
        assert_eq!(bare.to_deb(), "bun");
        assert_eq!(bare.to_pacman(), "bun");
    }

    #[test]
    fn provides_rejects_range() {
        assert_eq!(
            VirtualPackage::parse("bun>=1.0"),
            Err(RelationError::NonEqualProvides)
        );
        assert_eq!(
            VirtualPackage::parse("bun<2"),
            Err(RelationError::NonEqualProvides)
        );
    }
}
