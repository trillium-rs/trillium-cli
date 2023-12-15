use std::{
    env,
    fmt::{self, Debug, Display},
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub struct RootPath(PathBuf);
impl Default for RootPath {
    fn default() -> Self {
        Self(
            env::current_dir()
                .expect("current dir")
                .canonicalize()
                .expect("canonicalize"),
        )
    }
}

impl std::ops::Deref for RootPath {
    type Target = PathBuf;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for RootPath {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Debug for RootPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Debug::fmt(&self.0, f)
    }
}

impl Display for RootPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.to_str().unwrap())
    }
}

impl From<&std::ffi::OsStr> for RootPath {
    fn from(s: &std::ffi::OsStr) -> Self {
        Self(PathBuf::from(s).canonicalize().expect("canonicalize"))
    }
}

impl AsRef<PathBuf> for RootPath {
    fn as_ref(&self) -> &PathBuf {
        self
    }
}

impl AsRef<Path> for RootPath {
    fn as_ref(&self) -> &Path {
        self
    }
}

impl From<RootPath> for PathBuf {
    fn from(root: RootPath) -> Self {
        root.0
    }
}
